//! Der nächste Formatwechsel als Diff: which of the operator's own messages a
//! candidate release (a Konsultationsfassung) would refuse.
//!
//! **The corpus.** makod keeps no full copy of an exchanged message: the event
//! store holds the domain event with its `Verlauf` (direction, PID, document
//! number), the AS4 evidence store holds each leg's `AuditRecord` (signer,
//! signed digests), and the outbox renders EDIFACT at send time without
//! keeping the bytes. So the corpus is the operator's own interchange archive
//! — the § 147 AO record kept beside makod (an AS4 gateway's archive, say) —
//! one interchange per file, named per Allgemeine Festlegungen 6.1d Kap. 2.12
//! `<MsgType>_<SenderMPID>_<ReceiverMPID>_<YYMMDD>_<HHMM>_<Ref>.txt`. The name
//! gives the message type, the counterparty, the direction (the sender is one
//! of the operator's own MP-IDs → `ausgang`) and the date the window bounds.
//! A `.txt.gz` file and a file the convention does not name are counted as not
//! read, never silently skipped. Nothing read here leaves the deployment.
//!
//! **The comparison.** Each message is validated by the profile validator
//! ingest runs, under the binding profile ingest selects for it and under the
//! candidate ([`edi_energy::candidate`]); only a refusal the candidate adds is
//! listed. A Prüfidentifikator `cargo xtask profile-diff` does not report
//! changed is not re-validated, and a message type no candidate covers is
//! counted, not read.
//!
//! `impact_route` is `GET /api/v1/format-candidates/{fv}/impact` (JSON, or
//! CSV on `?format=csv` / `Accept: text/csv`) with its document entry, for
//! `OpenApiRouter`'s `routes!`; `get_format_impact` is the same read for the
//! MCP tool of that name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, header};
use axum::response::{IntoResponse, Response};
use edi_energy::candidate::{Candidate, Comparison, NewRefusal, interchange_messages};
use edi_energy::profile::Parteirollen;
use edi_energy::{MessageType, Release, ReleaseRegistry};
use mako_service::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

/// `[format_impact]`: where the candidates and the archive are, and how far
/// back the report reads.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FormatImpactConfig {
    /// The candidates tree, `<type>/<fv>/{candidate,mig,ahb}.json`, as
    /// `cargo xtask import-profiles --candidate` writes it.
    pub candidates_dir: PathBuf,
    /// The operator's interchange archive, one interchange per file.
    pub archive_dir: PathBuf,
    /// How many days back from today the report reads.
    #[serde(default = "default_window_days")]
    pub window_days: u16,
}

const fn default_window_days() -> u16 {
    90
}

/// The read's state: the configuration and the operator's own MP-IDs, which
/// tell an outbound interchange from an inbound one.
#[derive(Debug, Clone)]
pub struct FormatImpactState {
    pub config: FormatImpactConfig,
    pub own_mp_ids: BTreeSet<String>,
}

/// The impact report of one candidate Formatversion.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Impact {
    /// The candidate's directory name, `fv<yyyymmdd>[_gas]`.
    pub fv: String,
    /// Every message type the candidate covers.
    pub candidates: Vec<CandidateInfo>,
    /// The days read.
    pub window: Window,
    /// Messages re-validated under both profiles.
    pub revalidated: usize,
    /// Each new refusal, one row per rule.
    pub refusals: Vec<ImpactRow>,
    /// Messages whose Prüfidentifikator the candidate removes or adds.
    pub pid_changes: Vec<PidChange>,
    /// Messages not re-validated because the candidate does not change their
    /// Prüfidentifikator, per PID (`ohne` for none).
    pub not_revalidated: BTreeMap<String, usize>,
    /// Interchanges of a message type no candidate covers, per type.
    pub message_types_not_covered: BTreeMap<String, usize>,
    /// Files not read, per reason (`compressed`, `unnamed`, `unparseable`,
    /// `no_binding_profile`).
    pub not_read: BTreeMap<String, usize>,
}

/// The first and last day read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Window {
    /// `YYYY-MM-DD`.
    pub from: String,
    /// `YYYY-MM-DD`, today.
    pub to: String,
}

/// One message type of the candidate.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct CandidateInfo {
    pub message_type: String,
    /// `Konsultationsfassung`.
    pub status: String,
    /// The binding Formatversion it is compared with.
    pub against: String,
    pub sources: Vec<SourceDoc>,
}

/// One consultation PDF.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SourceDoc {
    pub file: String,
    pub sha256: String,
}

/// One new refusal of one message.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ImpactRow {
    pub message_type: String,
    pub pid: Option<u32>,
    /// `eingang` or `ausgang`.
    pub richtung: &'static str,
    pub counterparty: String,
    /// `YYYY-MM-DD`, from the file name.
    pub date: String,
    pub file: String,
    pub message_ref: String,
    /// For an outbound message: what in mako builds it, since mako itself must
    /// change before the Stichtag.
    pub builder: Option<String>,
    /// The rule (`AHB-55001-00047-STS-MISSING`, `MIG-…`).
    pub rule_id: Option<String>,
    /// The segment tag the refusal sits on.
    pub segment: Option<String>,
    /// One-based position of the segment in its message, `UNH` = 1.
    pub segment_position: Option<u32>,
    /// The validator's sentence.
    pub message: String,
    /// The candidate AHB's status and operands for the place.
    pub rule_status: Vec<String>,
    /// Every Bedingung the place cites, with the candidate AHB's text.
    pub bedingungen: BTreeMap<String, String>,
}

/// A message whose Prüfidentifikator one of the two releases lacks.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PidChange {
    pub pid: u32,
    /// `removed` or `added`.
    pub change: &'static str,
    pub richtung: &'static str,
    pub counterparty: String,
    pub date: String,
    pub file: String,
}

/// Why the report could not be made.
#[derive(Debug)]
pub enum ImpactError {
    /// The `fv` is not `fv<yyyymmdd>[_gas]`.
    InvalidFv(String),
    /// No candidate of that Formatversion is imported.
    NotFound(String),
    /// A candidate that does not load.
    Invalid { dir: String, reason: String },
    /// The archive could not be listed.
    Archive { dir: String, reason: String },
}

impl std::fmt::Display for ImpactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidFv(fv) => write!(
                f,
                "{fv:?} is not a Formatversion directory name (fv<yyyymmdd>[_gas])"
            ),
            Self::NotFound(fv) => write!(f, "no candidate {fv} is imported"),
            Self::Invalid { dir, reason } => write!(f, "candidate {dir}: {reason}"),
            Self::Archive { dir, reason } => {
                write!(f, "the interchange archive {dir} cannot be read: {reason}")
            }
        }
    }
}

impl std::error::Error for ImpactError {}

impl From<ImpactError> for ApiError {
    fn from(e: ImpactError) -> Self {
        let msg = e.to_string();
        match e {
            ImpactError::InvalidFv(_) => {
                ApiError::bad_request(msg).with_code("invalid_format_version")
            }
            ImpactError::NotFound(_) => ApiError::not_found(msg).with_code("candidate_not_found"),
            ImpactError::Invalid { .. } => {
                ApiError::unprocessable(msg).with_code("candidate_invalid")
            }
            ImpactError::Archive { .. } => {
                ApiError::unavailable(msg).with_code("archive_unavailable")
            }
        }
    }
}

/// One archived interchange, read off its Kap. 2.12 file name.
#[derive(Debug, PartialEq, Eq)]
struct Archived {
    message_type: String,
    sender: String,
    receiver: String,
    date: time::Date,
}

fn archived(name: &str) -> Option<Archived> {
    let stem = name.strip_suffix(".txt")?;
    let mut parts = stem.splitn(6, '_');
    let message_type = parts.next()?.to_ascii_uppercase();
    let sender = parts.next()?.to_owned();
    let receiver = parts.next()?.to_owned();
    let yymmdd = parts.next()?;
    let _hhmm = parts.next()?;
    parts.next()?;
    if yymmdd.len() != 6 || !yymmdd.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n = |r: std::ops::Range<usize>| yymmdd[r].parse::<u8>().ok();
    let date = time::Date::from_calendar_date(
        2000 + i32::from(n(0..2)?),
        time::Month::try_from(n(2..4)?).ok()?,
        n(4..6)?,
    )
    .ok()?;
    Some(Archived {
        message_type,
        sender,
        receiver,
        date,
    })
}

fn valid_fv(fv: &str) -> bool {
    let digits = fv
        .strip_prefix("fv")
        .map(|r| r.strip_suffix("_gas").unwrap_or(r));
    digits.is_some_and(|d| d.len() == 8 && d.bytes().all(|b| b.is_ascii_digit()))
}

fn iso(d: time::Date) -> String {
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// The impact report of candidate `fv` over the archive, read back from
/// `today` — the read the route and the MCP tool `get_format_impact` serve.
///
/// # Errors
///
/// An invalid or unknown `fv`, a candidate that does not load, or an archive
/// that cannot be listed.
pub fn get_format_impact(
    state: &FormatImpactState,
    fv: &str,
    today: time::Date,
) -> Result<Impact, ImpactError> {
    if !valid_fv(fv) {
        return Err(ImpactError::InvalidFv(fv.to_owned()));
    }
    let cfg = &state.config;
    let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
    for ty in std::fs::read_dir(&cfg.candidates_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let dir = ty.path().join(fv);
        if dir.join("candidate.json").is_file() {
            let c = Candidate::load_dir(&dir).map_err(|e| ImpactError::Invalid {
                dir: dir.display().to_string(),
                reason: e.to_string(),
            })?;
            candidates.insert(c.manifest().message_type.clone(), c);
        }
    }
    if candidates.is_empty() {
        return Err(ImpactError::NotFound(fv.to_owned()));
    }
    let from = today - time::Duration::days(i64::from(cfg.window_days));
    let mut impact = Impact {
        fv: fv.to_owned(),
        candidates: candidates
            .values()
            .map(|c| {
                let m = c.manifest();
                CandidateInfo {
                    message_type: m.message_type.clone(),
                    status: m.status.clone(),
                    against: m.against.clone(),
                    sources: m
                        .sources
                        .iter()
                        .map(|s| SourceDoc {
                            file: s.file.clone(),
                            sha256: s.sha256.clone(),
                        })
                        .collect(),
                }
            })
            .collect(),
        window: Window {
            from: iso(from),
            to: iso(today),
        },
        revalidated: 0,
        refusals: Vec::new(),
        pid_changes: Vec::new(),
        not_revalidated: BTreeMap::new(),
        message_types_not_covered: BTreeMap::new(),
        not_read: BTreeMap::new(),
    };
    let mut files = Vec::new();
    archive_files(&cfg.archive_dir, &mut files).map_err(|e| ImpactError::Archive {
        dir: cfg.archive_dir.display().to_string(),
        reason: e.to_string(),
    })?;
    files.sort();
    let mut not_read = |why: &str| *impact.not_read.entry(why.to_owned()).or_default() += 1;
    let mut read = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.ends_with(".gz") {
            if archived(name.trim_end_matches(".gz"))
                .is_some_and(|a| a.date >= from && a.date <= today)
            {
                not_read("compressed");
            }
            continue;
        }
        let Some(a) = archived(&name) else {
            not_read("unnamed");
            continue;
        };
        if a.date < from || a.date > today {
            continue;
        }
        read.push((path, name, a));
    }
    for (path, name, a) in read {
        let Some(candidate) = candidates.get(&a.message_type) else {
            *impact
                .message_types_not_covered
                .entry(a.message_type.clone())
                .or_default() += 1;
            continue;
        };
        let Ok(messages) = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|b| interchange_messages(&b).map_err(|e| e.to_string()))
        else {
            *impact.not_read.entry("unparseable".to_owned()).or_default() += 1;
            continue;
        };
        let ausgang = state.own_mp_ids.contains(&a.sender);
        let (richtung, counterparty) = if ausgang {
            ("ausgang", a.receiver.clone())
        } else {
            ("eingang", a.sender.clone())
        };
        for message in messages {
            let binding = MessageType::from_unh_code(&message.message_type).and_then(|mt| {
                ReleaseRegistry::global()
                    .profile_on(mt, &Release::new(&message.release), a.date)
                    .ok()
            });
            let Some(binding) = binding else {
                *impact
                    .not_read
                    .entry("no_binding_profile".to_owned())
                    .or_default() += 1;
                continue;
            };
            let change = |pid, change| PidChange {
                pid,
                change,
                richtung,
                counterparty: counterparty.clone(),
                date: iso(a.date),
                file: name.clone(),
            };
            match candidate.compare(binding, &message, &Parteirollen::unbekannt()) {
                Comparison::NotRevalidated { pid } => {
                    let key = pid.map_or_else(|| "ohne".to_owned(), |p| p.to_string());
                    *impact.not_revalidated.entry(key).or_default() += 1;
                }
                Comparison::PidRemoved { pid } => impact.pid_changes.push(change(pid, "removed")),
                Comparison::PidAdded { pid } => impact.pid_changes.push(change(pid, "added")),
                Comparison::Unchanged => impact.revalidated += 1,
                Comparison::Worse(refusals) => {
                    impact.revalidated += 1;
                    for refusal in refusals {
                        impact.refusals.push(ImpactRow {
                            message_type: message.message_type.clone(),
                            pid: message.pid,
                            richtung,
                            counterparty: counterparty.clone(),
                            date: iso(a.date),
                            file: name.clone(),
                            message_ref: message.message_ref.clone(),
                            builder: ausgang.then(|| {
                                format!("makod edifact_renderer ({})", message.message_type)
                            }),
                            ..row(refusal)
                        });
                    }
                }
            }
        }
    }
    Ok(impact)
}

/// The refusal half of an [`ImpactRow`]; the message half is the caller's.
fn row(refusal: NewRefusal) -> ImpactRow {
    let (rule_status, bedingungen) = refusal
        .regel
        .map(|r| (r.status, r.bedingungen))
        .unwrap_or_default();
    ImpactRow {
        message_type: String::new(),
        pid: None,
        richtung: "",
        counterparty: String::new(),
        date: String::new(),
        file: String::new(),
        message_ref: String::new(),
        builder: None,
        rule_id: refusal.rule_id,
        segment: refusal.segment,
        segment_position: refusal.segment_position,
        message: refusal.message,
        rule_status,
        bedingungen,
    }
}

fn archive_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            archive_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// The report as CSV for the IT provider or the counterparty: one row per new
/// refusal, `;`-separated as a German spreadsheet reads it.
#[must_use]
pub fn csv(impact: &Impact) -> String {
    let field = |s: &str| {
        if s.contains([';', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_owned()
        }
    };
    let mut out = String::from(
        "Nachrichtentyp;PID;Richtung;Marktpartner;Datum;Datei;Nachrichtenreferenz;Regel;Segment;Position;Meldung;Ersteller\n",
    );
    for r in &impact.refusals {
        let cells = [
            r.message_type.clone(),
            r.pid.map(|p| p.to_string()).unwrap_or_default(),
            r.richtung.to_owned(),
            r.counterparty.clone(),
            r.date.clone(),
            r.file.clone(),
            r.message_ref.clone(),
            r.rule_id.clone().unwrap_or_default(),
            r.segment.clone().unwrap_or_default(),
            r.segment_position
                .map(|p| p.to_string())
                .unwrap_or_default(),
            r.message.clone(),
            r.builder.clone().unwrap_or_default(),
        ];
        let line: Vec<String> = cells.iter().map(|c| field(c)).collect();
        out.push_str(&line.join(";"));
        out.push('\n');
    }
    out
}

/// `?format=csv`.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ImpactQuery {
    /// `json` (the default) or `csv`.
    #[serde(default)]
    pub format: Option<String>,
}

/// Which of the operator's archived messages candidate `fv` would newly
/// refuse.
///
/// # Errors
///
/// `invalid_format_version`, `candidate_not_found`, `candidate_invalid`,
/// `archive_unavailable`, `invalid_format`.
#[utoipa::path(
    get,
    path = "/api/v1/format-candidates/{fv}/impact",
    tag = "format-candidates",
    params(
        ("fv" = String, Path, description = "The candidate's directory name, fv<yyyymmdd>[_gas]"),
        ImpactQuery,
    ),
    responses(
        (status = 200, description = "The impact report; text/csv on ?format=csv or Accept: text/csv", body = Impact),
        (status = 400, description = "invalid_format_version or invalid_format"),
        (status = 404, description = "candidate_not_found"),
        (status = 422, description = "candidate_invalid"),
        (status = 503, description = "archive_unavailable"),
    ),
    security(("bearer_token" = []))
)]
pub async fn impact_route(
    State(state): State<Arc<FormatImpactState>>,
    UrlPath(fv): UrlPath<String>,
    Query(q): Query<ImpactQuery>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let wants_csv = match q.format.as_deref() {
        Some("csv") => true,
        Some("json") | None => headers
            .get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/csv")),
        Some(other) => {
            return Err(
                ApiError::bad_request(format!("format {other:?}: csv or json"))
                    .with_code("invalid_format"),
            );
        }
    };
    let today = mako_fristen::heute();
    let impact = tokio::task::spawn_blocking(move || get_format_impact(&state, &fv, today))
        .await
        .map_err(ApiError::internal)??;
    Ok(if wants_csv {
        (
            [(header::CONTENT_TYPE, "text/csv; charset=utf-8")],
            csv(&impact),
        )
            .into_response()
    } else {
        axum::Json(impact).into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use edi_energy::profile::SkeletonParties;
    use serde_json::{Value, json};

    const MIG: &str =
        include_str!("../../../crates/edi-energy/profiles/utilmd/fv20261001/mig.json");
    const AHB: &str =
        include_str!("../../../crates/edi-energy/profiles/utilmd/fv20261001/ahb.json");
    const OWN: &str = "9900357000004";

    fn today() -> time::Date {
        time::Date::from_calendar_date(2026, time::Month::November, 17).expect("a date")
    }

    /// The binding UTILMD AHB with the first choice of codes on 55001 cut.
    fn tightened() -> String {
        let mut ahb: Value = serde_json::from_str(AHB).expect("parses");
        let af = ahb["anwendungsfaelle"]
            .as_array_mut()
            .expect("columns")
            .iter_mut()
            .find(|a| a["pid"] == 55001)
            .expect("55001");
        let rule = af["elements"]
            .as_array_mut()
            .expect("elements")
            .iter_mut()
            .find(|e| {
                e["operands"]
                    .as_array()
                    .is_some_and(|o| o.iter().filter(|o| o["code"].is_string()).count() >= 2)
            })
            .expect("a choice of codes");
        let ops = rule["operands"].as_array_mut().expect("operands");
        let at = ops
            .iter()
            .position(|o| o["code"].is_string())
            .expect("a code");
        ops.remove(at);
        ahb.to_string()
    }

    fn setup(name: &str) -> (tempfile::TempDir, FormatImpactState) {
        let root = tempfile::tempdir().expect("tempdir");
        let cand = root.path().join("candidates/utilmd/fv20270401");
        std::fs::create_dir_all(&cand).expect("dir");
        std::fs::write(cand.join("mig.json"), MIG).expect("write");
        std::fs::write(cand.join("ahb.json"), tightened()).expect("write");
        std::fs::write(
            cand.join("candidate.json"),
            json!({
                "message_type": "UTILMD", "fv": "fv20270401",
                "status": "Konsultationsfassung", "against": "fv20261001",
                "sources": [{ "file": "UTILMD_AHB_Konsultation.pdf", "sha256": "b".repeat(64) }],
                "changed_pids": [55001], "added_pids": [], "removed_pids": [],
            })
            .to_string(),
        )
        .expect("write");
        let archive = root.path().join("archive");
        std::fs::create_dir_all(&archive).expect("dir");
        let binding = ReleaseRegistry::global()
            .profile_on(MessageType::Utilmd, &Release::new("S2.2"), today())
            .expect("UTILMD S2.2 is embedded");
        let skeleton = |pid| {
            binding
                .skeleton_interchange(
                    binding.anwendungsfall(pid).expect("column"),
                    &SkeletonParties::default(),
                )
                .expect("writes")
        };
        let put =
            |file: &str, bytes: &[u8]| std::fs::write(archive.join(file), bytes).expect("write");
        // Outbound 55001, refused by the tightened candidate.
        put(
            &format!("UTILMD_{OWN}_9900000000003_261110_0700_1.txt"),
            &skeleton(55001),
        );
        // Inbound 55002: a PID the candidate does not change.
        put(
            &format!("UTILMD_9900000000003_{OWN}_261111_0800_2.txt"),
            &skeleton(55002),
        );
        // Outside the window: not read at all.
        put(
            &format!("UTILMD_{OWN}_9900000000003_261005_0700_3.txt"),
            &skeleton(55001),
        );
        // Compressed and unnamed: counted, not read.
        put(
            &format!("UTILMD_{OWN}_9900000000003_261112_0700_4.txt.gz"),
            b"\x1f\x8b",
        );
        put("notes.txt", b"");
        // A message type no candidate covers.
        put(
            &format!("MSCONS_9900000000003_{OWN}_261113_0700_5.txt"),
            b"",
        );
        let _ = name;
        let state = FormatImpactState {
            config: FormatImpactConfig {
                candidates_dir: root.path().join("candidates"),
                archive_dir: archive,
                window_days: 30,
            },
            own_mp_ids: [OWN.to_owned()].into(),
        };
        (root, state)
    }

    /// Over the archive, exactly the outbound 55001 interchange in the
    /// window is listed, with the counterparty, the builder and the rule; the
    /// rest is counted by why it was not.
    #[test]
    fn the_impact_lists_the_archived_messages_the_candidate_refuses() {
        let (_root, state) = setup("impact");
        let impact = get_format_impact(&state, "fv20270401", today()).expect("the report");
        let files: BTreeSet<&str> = impact.refusals.iter().map(|r| r.file.as_str()).collect();
        assert_eq!(
            files,
            [format!("UTILMD_{OWN}_9900000000003_261110_0700_1.txt").as_str()].into(),
            "{impact:#?}"
        );
        let row = &impact.refusals[0];
        assert_eq!(
            (row.richtung, row.counterparty.as_str()),
            ("ausgang", "9900000000003")
        );
        assert_eq!(row.pid, Some(55001));
        assert!(
            row.builder.is_some(),
            "an outbound message names its builder"
        );
        assert!(
            row.rule_id
                .as_deref()
                .is_some_and(|r| r.starts_with("AHB-55001-"))
        );
        assert!(
            !row.rule_status.is_empty(),
            "the candidate AHB's words: {row:?}"
        );
        assert_eq!(impact.not_revalidated.get("55002"), Some(&1));
        assert_eq!(impact.not_read.get("compressed"), Some(&1));
        assert_eq!(impact.not_read.get("unnamed"), Some(&1));
        assert_eq!(impact.message_types_not_covered.get("MSCONS"), Some(&1));
        assert_eq!(
            impact.not_read.get("no_binding_profile"),
            None,
            "{impact:#?}"
        );
        assert_eq!(
            impact.window,
            Window {
                from: "2026-10-18".to_owned(),
                to: "2026-11-17".to_owned()
            }
        );

        let csv = csv(&impact);
        assert_eq!(csv.lines().count(), 1 + impact.refusals.len());
        assert!(
            csv.lines()
                .nth(1)
                .is_some_and(|l| l.starts_with("UTILMD;55001;ausgang;9900000000003;2026-11-10;"))
        );
    }

    #[test]
    fn an_unknown_or_malformed_candidate_is_refused_with_its_code() {
        let (_root, state) = setup("refused");
        let code = |fv: &str| {
            ApiError::from(get_format_impact(&state, fv, today()).expect_err("refused"))
                .code()
                .map(str::to_owned)
        };
        assert_eq!(code("fv20280401").as_deref(), Some("candidate_not_found"));
        assert_eq!(
            code("../profiles").as_deref(),
            Some("invalid_format_version")
        );
    }

    #[test]
    fn a_kap_2_12_file_name_names_the_interchange() {
        assert_eq!(
            archived("UTILMD_9900357000004_9900000000003_261110_0700_ABC_1.txt"),
            Some(Archived {
                message_type: "UTILMD".into(),
                sender: "9900357000004".into(),
                receiver: "9900000000003".into(),
                date: time::Date::from_calendar_date(2026, time::Month::November, 10)
                    .expect("date"),
            })
        );
        assert_eq!(archived("UTILMD_1_2_261310_0700_1.txt"), None);
        assert_eq!(archived("readme.txt"), None);
    }
}
