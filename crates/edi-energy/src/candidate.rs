//! A candidate release: a Konsultationsfassung loaded beside the binding
//! profiles to ask which messages the next Formatversion would refuse.
//!
//! A candidate is never binding (the binding edition is the source of the
//! Prüfschablone): it is loaded at runtime from a directory the caller names,
//! never embedded by `build.rs`, and never registered into a
//! [`ReleaseRegistry`](crate::ReleaseRegistry), so ingest cannot select it. Its
//! `candidate.json` names it a `Konsultationsfassung`, records the SHA-256 of
//! every consultation PDF it was imported from, and lists the
//! Prüfidentifikatoren `cargo xtask profile-diff` reports changed against the
//! binding Formatversion it is compared with.
//!
//! The comparison is between **verdicts** on one message, never between the
//! two documents: the message is validated by the same profile validator as
//! ingest under its binding profile and under the candidate, and only a
//! refusal the candidate adds is reported.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use edifact::{OwnedSegment, ValidationIssue};
use serde::{Deserialize, Serialize};

use crate::profile::Parteirollen;
use crate::profile::validate::bars_sending;
use crate::registry::PidSource;
use crate::{Error, Profile, ProfileError};

/// The one status a candidate carries.
pub const KONSULTATIONSFASSUNG: &str = "Konsultationsfassung";

/// One consultation document a candidate was imported from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateSource {
    /// The file name in the document mirror.
    pub file: String,
    /// SHA-256 of the file, lower-case hex.
    pub sha256: String,
}

/// `candidate.json`: what a candidate is and what it changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateManifest {
    /// `UTILMD`, `MSCONS`, … as in `UNH` DE 0065.
    pub message_type: String,
    /// The candidate's directory name, `fv<yyyymmdd>[_gas]`.
    pub fv: String,
    /// Always [`KONSULTATIONSFASSUNG`].
    pub status: String,
    /// The binding Formatversion it is compared with, `fv<yyyymmdd>[_gas]`.
    pub against: String,
    /// The consultation PDFs, each with its SHA-256.
    pub sources: Vec<CandidateSource>,
    /// Prüfidentifikatoren whose Prüfschablone differs from `against`.
    pub changed_pids: Vec<u32>,
    /// Prüfidentifikatoren the candidate adds.
    pub added_pids: Vec<u32>,
    /// Prüfidentifikatoren the candidate removes.
    pub removed_pids: Vec<u32>,
}

impl CandidateManifest {
    /// Whether the manifest names a Konsultationsfassung — the only thing a
    /// candidate may be.
    #[must_use]
    pub fn is_candidate(&self) -> bool {
        self.status == KONSULTATIONSFASSUNG
    }

    fn check(&self) -> Result<(), ProfileError> {
        let invalid = |field: &'static str, value: &str, reason: &str| ProfileError::InvalidField {
            field,
            value: value.to_owned(),
            reason: reason.to_owned(),
        };
        if !self.is_candidate() {
            return Err(invalid(
                "status",
                &self.status,
                "a candidate is a Konsultationsfassung and never binding",
            ));
        }
        if self.sources.is_empty() {
            return Err(invalid(
                "sources",
                "",
                "a candidate names its consultation PDFs",
            ));
        }
        for s in &self.sources {
            if s.sha256.len() != 64 || !s.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(invalid("sources.sha256", &s.sha256, "not a SHA-256"));
            }
        }
        Ok(())
    }
}

/// A loaded candidate: its manifest and its profile.
#[derive(Debug)]
pub struct Candidate {
    manifest: CandidateManifest,
    profile: Profile,
}

impl Candidate {
    /// Load a candidate from `candidate.json`, `mig.json` and `ahb.json`.
    ///
    /// # Errors
    ///
    /// When a document does not parse, the manifest names no
    /// Konsultationsfassung or a source without a SHA-256, or the manifest and
    /// the profile disagree on the message type.
    pub fn from_json(candidate: &str, mig: &str, ahb: &str) -> Result<Self, ProfileError> {
        let manifest: CandidateManifest =
            serde_json::from_str(candidate).map_err(|e| ProfileError::InvalidField {
                field: "candidate.json",
                value: String::new(),
                reason: e.to_string(),
            })?;
        manifest.check()?;
        let profile = Profile::from_json(mig, ahb)?;
        if profile.message_type().as_str() != manifest.message_type {
            return Err(ProfileError::InvalidField {
                field: "message_type",
                value: manifest.message_type,
                reason: format!("the profile is {}", profile.message_type()),
            });
        }
        Ok(Self { manifest, profile })
    }

    /// Load the candidate in `dir` (`candidate.json`, `mig.json`, `ahb.json`).
    ///
    /// # Errors
    ///
    /// A file that cannot be read, and everything [`Self::from_json`] refuses.
    pub fn load_dir(dir: &Path) -> Result<Self, ProfileError> {
        let read = |name: &'static str| {
            std::fs::read_to_string(dir.join(name)).map_err(|e| ProfileError::InvalidField {
                field: name,
                value: dir.display().to_string(),
                reason: e.to_string(),
            })
        };
        Self::from_json(
            &read("candidate.json")?,
            &read("mig.json")?,
            &read("ahb.json")?,
        )
    }

    /// The manifest.
    #[must_use]
    pub fn manifest(&self) -> &CandidateManifest {
        &self.manifest
    }

    /// The candidate's profile — for comparison only, never for ingest.
    #[must_use]
    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The verdict of `binding` and of the candidate on `message`, compared.
    ///
    /// `binding` is the profile ingest selects for the message (its wire
    /// release on its date). A Prüfidentifikator one of the two profiles lacks
    /// is reported apart; one outside `changed_pids` is not re-validated.
    #[must_use]
    pub fn compare(
        &self,
        binding: &Profile,
        message: &InterchangeMessage,
        rollen: &Parteirollen,
    ) -> Comparison {
        let pid = message.pid;
        // Read off the two profiles, not the manifest's lists: a column the
        // candidate lacks has no verdict to compare.
        if let Some(p) = pid {
            let (was, is) = (
                binding.anwendungsfall(p).is_some(),
                self.profile.anwendungsfall(p).is_some(),
            );
            if was && !is {
                return Comparison::PidRemoved { pid: p };
            }
            if is && !was {
                return Comparison::PidAdded { pid: p };
            }
        }
        if !pid.is_some_and(|p| self.manifest.changed_pids.contains(&p)) {
            return Comparison::NotRevalidated { pid };
        }
        let typed = pid.and_then(|p| crate::Pruefidentifikator::new(p).ok());
        let verdict = |profile: &Profile| -> Vec<ValidationIssue> {
            profile
                .validate_with_rollen(&message.segments, typed, rollen, message.decimal_mark)
                .into_iter()
                .filter(bars_sending)
                .collect()
        };
        let before: BTreeSet<(Option<String>, Option<String>)> =
            verdict(binding).iter().map(key).collect();
        let refusals: Vec<NewRefusal> = verdict(&self.profile)
            .into_iter()
            .filter(|i| !before.contains(&key(i)))
            .map(|i| self.refusal(pid, i))
            .collect();
        if refusals.is_empty() {
            Comparison::Unchanged
        } else {
            Comparison::Worse(refusals)
        }
    }

    fn refusal(&self, pid: Option<u32>, issue: ValidationIssue) -> NewRefusal {
        NewRefusal {
            regel: issue
                .rule_id
                .as_deref()
                .and_then(|r| self.rule_text(pid?, r)),
            rule_id: issue.rule_id,
            segment: issue.segment_tag,
            segment_position: issue.segment_position,
            message: issue.message,
        }
    }

    /// The candidate AHB's words for the place a rule id names
    /// (`AHB-<pid>-<nr>-…`): the status as printed and every Bedingung it
    /// cites, with its text.
    fn rule_text(&self, pid: u32, rule_id: &str) -> Option<RuleText> {
        let nr = rule_id
            .strip_prefix(&format!("AHB-{pid}-"))?
            .split('-')
            .next()?;
        let af = self.profile.anwendungsfall(pid)?;
        let status: Vec<String> = af
            .rows
            .iter()
            .filter(|r| r.nr.as_deref() == Some(nr) || r.before.as_deref() == Some(nr))
            .flat_map(|r| r.status.iter().cloned())
            .chain(
                af.elements
                    .iter()
                    .filter(|e| e.nr == nr)
                    .flat_map(|e| e.operands.iter().map(|o| o.operand.clone())),
            )
            .collect();
        let mut bedingungen = BTreeMap::new();
        for expr in &status {
            for id in cited(expr) {
                if let Some(text) = self.profile.ahb.conditions.get(&id) {
                    bedingungen.insert(id, text.clone());
                }
            }
        }
        Some(RuleText {
            status,
            bedingungen,
        })
    }
}

fn key(issue: &ValidationIssue) -> (Option<String>, Option<String>) {
    (issue.rule_id.clone(), issue.segment_tag.clone())
}

/// The `[n]` a status expression cites.
fn cited(expr: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = expr;
    while let Some(open) = rest.find('[') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find(']') else { break };
        out.push(rest[..close].to_owned());
        rest = &rest[close + 1..];
    }
    out
}

/// What a candidate says about one message, compared with its binding profile.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Comparison {
    /// The candidate does not change the message's Prüfidentifikator: not
    /// re-validated.
    NotRevalidated {
        /// The message's Prüfidentifikator, where it carries one.
        pid: Option<u32>,
    },
    /// The candidate removes the Prüfidentifikator.
    PidRemoved {
        /// The removed Prüfidentifikator.
        pid: u32,
    },
    /// The candidate adds the Prüfidentifikator, which the binding release
    /// does not carry.
    PidAdded {
        /// The added Prüfidentifikator.
        pid: u32,
    },
    /// Re-validated, and refused by nothing the binding profile did not
    /// refuse.
    Unchanged,
    /// Refusals the candidate adds.
    Worse(Vec<NewRefusal>),
}

/// One refusal the candidate adds.
#[derive(Debug, Clone, Serialize)]
pub struct NewRefusal {
    /// The rule (`AHB-55001-00047-STS-MISSING`, `MIG-…`).
    pub rule_id: Option<String>,
    /// The segment tag the refusal sits on.
    pub segment: Option<String>,
    /// One-based position of the segment in its message, `UNH` = 1.
    pub segment_position: Option<u32>,
    /// The validator's sentence.
    pub message: String,
    /// The candidate AHB's words for the place, where the rule names one.
    pub regel: Option<RuleText>,
}

/// The candidate AHB's words for one place.
#[derive(Debug, Clone, Serialize)]
pub struct RuleText {
    /// The status and operands as printed (`Muss [10]`, `X [931]`).
    pub status: Vec<String>,
    /// Every Bedingung cited, with its text.
    pub bedingungen: BTreeMap<String, String>,
}

/// One message (`UNH` … `UNT`) out of an interchange.
#[derive(Debug, Clone)]
pub struct InterchangeMessage {
    /// `UNH` DE 0065.
    pub message_type: String,
    /// `UNH` DE 0057, the wire release.
    pub release: String,
    /// `UNH` DE 0062, the message reference.
    pub message_ref: String,
    /// The Prüfidentifikator, where the message carries one.
    pub pid: Option<u32>,
    /// The decimal mark the interchange's `UNA` declares.
    pub decimal_mark: char,
    /// The message's segments, `UNH` … `UNT`.
    pub segments: Vec<OwnedSegment>,
}

/// Every message of an interchange, for comparison.
///
/// # Errors
///
/// Bytes that are no EDIFACT interchange.
pub fn interchange_messages(bytes: &[u8]) -> Result<Vec<InterchangeMessage>, Error> {
    let decimal_mark = crate::una_decimal_mark(bytes);
    let decoded = edifact::decode_interchange(bytes).map_err(Error::Parse)?;
    let cfg = crate::ParseConfig::default().to_reader_config();
    let segments: Vec<OwnedSegment> = edifact::from_reader_with_config(decoded.as_ref(), cfg)
        .collect::<Result<_, _>>()
        .map_err(Error::Parse)?;
    let mut out = Vec::new();
    let mut current: Option<Vec<OwnedSegment>> = None;
    for segment in segments {
        match &*segment.tag {
            "UNH" => current = Some(vec![segment]),
            "UNT" => {
                if let Some(mut m) = current.take() {
                    m.push(segment);
                    out.push(message(m, decimal_mark));
                }
            }
            _ => {
                if let Some(m) = current.as_mut() {
                    m.push(segment);
                }
            }
        }
    }
    Ok(out)
}

fn message(segments: Vec<OwnedSegment>, decimal_mark: char) -> InterchangeMessage {
    let unh = &segments[0];
    let part = |e: usize, c: usize| unh.component_str(e, c).unwrap_or_default().to_owned();
    let pid = crate::pid_scan::detect(&segments, PidSource::BgmDe1004);
    InterchangeMessage {
        message_type: part(1, 0),
        release: part(1, 4),
        message_ref: unh.element_str(0).unwrap_or_default().to_owned(),
        pid,
        decimal_mark,
        segments,
    }
}
