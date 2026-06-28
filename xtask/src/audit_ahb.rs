//! `cargo xtask audit-ahb`
//!
//! Comprehensive AHB rule coverage analysis for all committed profiles.
//!
//! For every profile in `profiles/<type>/<release>/`, this task computes:
//! - **Segment coverage**: how many MIG-defined segments have at least one AHB
//!   rule in at least one PID (rule presence rate, not completeness).
//! - **Per-PID rule density**: average segment_rules + group_rules per PID.
//! - **Mandatory-group coverage**: mandatory MIG groups that have no group_rule
//!   in any PID — these are structural gaps.
//! - **Uncovered segments**: MIG segments absent from every PID's segment_rules.
//! - **Conditional rule count**: total conditional_rules across all PIDs (proxy
//!   for business-logic coverage depth).
//!
//! # Output
//!
//! Human-readable table to stderr + optional JSON report (--output-json `<PATH>`).
//!
//! # Exit codes
//!
//! - 0: all checks pass (or no threshold set)
//! - 1: at least one profile falls below --min-density or --min-cond-rules
//!
//! # Options
//!
//! ```
//! --message-type <TYPE>   Limit audit to a single message type (e.g. UTILMD)
//! --output-json  <PATH>   Write machine-readable JSON report to a file
//! --min-density  <N>      Fail if avg rules/PID < N for any active profile
//!                         (default: 0, disabled)
//!                         Recommended baseline: 0.4
//!                         Enable in CI: --min-density 0.4
//! --min-cond-rules <N>    Fail if total conditional_rules < N for any active
//!                         profile (default: 0, disabled)
//! --report-unmodelled     Print segment/group rules that have empty
//!                         conditional_rules but a _description note explaining
//!                         why the rule is not yet modelled. These are known
//!                         AHB gaps requiring future work.
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::Serialize;

// ── JSON models ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MigProfile {
    #[allow(dead_code)]
    message_type: String,
    #[allow(dead_code)]
    release: String,
    #[serde(default)]
    archived: bool,
    /// Profiles that intentionally have no PIDs (e.g. CONTRL).
    #[serde(default)]
    pid_exempt: bool,
    segments: Vec<MigSegment>,
    segment_groups: Vec<MigGroup>,
}

#[derive(Deserialize)]
struct MigSegment {
    tag: String,
}

#[derive(Deserialize)]
struct MigGroup {
    id: String,
    trigger_segment: String,
    mandatory: bool,
    segments: Vec<MigSegment>,
    groups: Vec<MigGroup>,
}

#[derive(Deserialize)]
struct AhbProfile {
    #[allow(dead_code)]
    message_type: String,
    #[allow(dead_code)]
    release: String,
    #[serde(default)]
    pruefidentifikatoren: Vec<AhbPruefidentifikator>,
}

#[derive(Deserialize)]
struct AhbPruefidentifikator {
    code: u32,
    #[allow(dead_code)]
    name: String,
    #[serde(default)]
    segment_rules: Vec<AhbSegmentRule>,
    #[serde(default)]
    group_rules: Vec<AhbGroupRule>,
}

#[derive(Deserialize)]
struct AhbSegmentRule {
    tag: String,
    #[allow(dead_code)]
    requirement: String,
    #[serde(default)]
    conditional_rules: Vec<serde_json::Value>,
    /// Free-text note explaining why no conditional rule is modelled (starts
    /// with `_` so it is treated as a JSON "comment field" by convention).
    #[serde(rename = "_description", default)]
    description: Option<String>,
}

#[derive(Deserialize)]
struct AhbGroupRule {
    group_id: String,
    #[allow(dead_code)]
    tag: String,
    #[serde(default)]
    conditional_rules: Vec<serde_json::Value>,
    #[serde(rename = "_description", default)]
    description: Option<String>,
}

// ── Report structures (JSON-serializable) ────────────────────────────────────

/// A segment or group rule with an empty `conditional_rules` but a `_description`
/// explaining why no rule is modelled. Surfaced by `--report-unmodelled`.
#[derive(Serialize, Clone)]
pub struct UnmodelledRule {
    /// PID that contains the unmodelled rule.
    pub pid: u32,
    /// Segment tag or group ID.
    pub id: String,
    /// The `_description` comment from the AHB JSON.
    pub description: String,
}

/// Full audit report — one entry per profile directory.
#[derive(Serialize)]
pub struct AuditReport {
    pub generated_at: String,
    pub profiles: Vec<ProfileReport>,
    pub summary: AuditSummary,
}

#[derive(Serialize, Clone)]
pub struct ProfileReport {
    pub message_type: String,
    pub release_dir: String,
    pub pid_count: usize,
    pub mig_segment_count: usize,
    pub total_segment_rules: usize,
    pub total_group_rules: usize,
    pub total_conditional_rules: usize,
    pub avg_rules_per_pid: f64,
    /// MIG segments with a segment_rule in at least one PID.
    pub covered_segments: Vec<String>,
    /// MIG segments absent from every PID's segment_rules.
    pub uncovered_segments: Vec<String>,
    /// Mandatory MIG groups with no group_rule in any PID.
    pub mandatory_groups_without_rules: Vec<MandatoryGroupGap>,
    /// Segment rules with empty `conditional_rules` and a `_description` note.
    /// These represent known AHB constraints that have not yet been encoded.
    /// Populated only when `--report-unmodelled` is passed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unmodelled_segment_rules: Vec<UnmodelledRule>,
    /// Group rules with empty `conditional_rules` and a `_description` note.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unmodelled_group_rules: Vec<UnmodelledRule>,
    /// Is the profile archived?
    pub archived: bool,
}

#[derive(Serialize, Clone)]
pub struct MandatoryGroupGap {
    pub group_id: String,
    pub trigger_segment: String,
}

#[derive(Serialize)]
pub struct AuditSummary {
    pub total_profiles: usize,
    pub total_pids: usize,
    pub total_segment_rules: usize,
    pub total_group_rules: usize,
    pub total_conditional_rules: usize,
    pub profiles_with_zero_pids: usize,
    pub profiles_below_density_threshold: Vec<String>,
    pub profiles_below_cond_rules_threshold: Vec<String>,
    /// Total segment + group rules that have empty `conditional_rules` but a
    /// `_description` note (only populated when `--report-unmodelled` is set).
    pub total_unmodelled_rules: usize,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run AHB coverage audit from `workspace_root`.  Returns `true` if all
/// threshold checks pass (or no thresholds are set).
pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let message_type_filter: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--message-type")
        .map(|w| w[1].to_uppercase());
    let output_json: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--output-json")
        .map(|w| w[1].clone());
    let min_density: f64 = args
        .windows(2)
        .find(|w| w[0] == "--min-density")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(0.0);
    let min_cond_rules: usize = args
        .windows(2)
        .find(|w| w[0] == "--min-cond-rules")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(0);
    let report_unmodelled: bool = args.iter().any(|a| a == "--report-unmodelled");

    let profiles_dir = PathBuf::from(workspace_root)
        .join("crates")
        .join("edi-energy")
        .join("profiles");

    let mut reports: Vec<ProfileReport> = Vec::new();

    for msg_dir in read_subdirs(&profiles_dir) {
        let msg_type = msg_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_uppercase();
        if msg_type == "SCHEMAS" {
            continue;
        }
        if let Some(ref filter) = message_type_filter
            && msg_type != *filter
        {
            continue;
        }

        for release_dir in read_subdirs(&msg_dir) {
            let rel_name = release_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            let mig_path = release_dir.join("mig.json");
            let ahb_path = release_dir.join("ahb.json");
            if !mig_path.exists() || !ahb_path.exists() {
                continue;
            }

            let mig: MigProfile = match load_json(&mig_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("ERROR  {msg_type}/{rel_name}/mig.json: {e}");
                    continue;
                }
            };
            let ahb: AhbProfile = match load_json(&ahb_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("ERROR  {msg_type}/{rel_name}/ahb.json: {e}");
                    continue;
                }
            };

            let report = analyse_profile(&msg_type, &rel_name, &mig, &ahb, report_unmodelled);
            reports.push(report);
        }
    }

    // Sort by message_type then release_dir for stable output.
    reports.sort_by(|a, b| {
        a.message_type
            .cmp(&b.message_type)
            .then(a.release_dir.cmp(&b.release_dir))
    });

    // ── Threshold checks ──────────────────────────────────────────────────────
    let mut below_density: Vec<String> = Vec::new();
    let mut below_cond: Vec<String> = Vec::new();

    for r in &reports {
        if r.archived || r.pid_count == 0 {
            continue;
        }
        if min_density > 0.0 && r.avg_rules_per_pid < min_density {
            below_density.push(format!("{}/{}", r.message_type, r.release_dir));
        }
        if min_cond_rules > 0 && r.total_conditional_rules < min_cond_rules {
            below_cond.push(format!("{}/{}", r.message_type, r.release_dir));
        }
    }

    // ── Summary ───────────────────────────────────────────────────────────────
    let total_pids: usize = reports.iter().map(|r| r.pid_count).sum();
    let total_seg_rules: usize = reports.iter().map(|r| r.total_segment_rules).sum();
    let total_grp_rules: usize = reports.iter().map(|r| r.total_group_rules).sum();
    let total_cond_rules: usize = reports.iter().map(|r| r.total_conditional_rules).sum();
    let zero_pids = reports
        .iter()
        .filter(|r| r.pid_count == 0 && !r.archived)
        .count();

    let summary = AuditSummary {
        total_profiles: reports.len(),
        total_pids,
        total_segment_rules: total_seg_rules,
        total_group_rules: total_grp_rules,
        total_conditional_rules: total_cond_rules,
        profiles_with_zero_pids: zero_pids,
        profiles_below_density_threshold: below_density.clone(),
        profiles_below_cond_rules_threshold: below_cond.clone(),
        total_unmodelled_rules: reports
            .iter()
            .map(|r| r.unmodelled_segment_rules.len() + r.unmodelled_group_rules.len())
            .sum(),
    };

    // ── Human-readable output ─────────────────────────────────────────────────
    print_table(&reports, &summary, min_density, min_cond_rules);

    // ── Unmodelled rule report ────────────────────────────────────────────────
    if report_unmodelled && summary.total_unmodelled_rules > 0 {
        eprintln!(
            "\nUnmodelled AHB rules ({} total — rules with empty conditional_rules + _description):",
            summary.total_unmodelled_rules
        );
        for r in &reports {
            if r.unmodelled_segment_rules.is_empty() && r.unmodelled_group_rules.is_empty() {
                continue;
            }
            eprintln!("  {}/{}", r.message_type, r.release_dir);
            for u in &r.unmodelled_segment_rules {
                eprintln!("    [seg] PID {:5}  {}  → {}", u.pid, u.id, u.description);
            }
            for u in &r.unmodelled_group_rules {
                eprintln!("    [grp] PID {:5}  {}  → {}", u.pid, u.id, u.description);
            }
        }
        eprintln!();
    } else if report_unmodelled {
        eprintln!("No unmodelled rules found.");
    }

    // ── JSON report ───────────────────────────────────────────────────────────
    if let Some(ref path) = output_json {
        let now = chrono_now_approx();
        let full_report = AuditReport {
            generated_at: now,
            profiles: reports,
            summary,
        };
        match serde_json::to_string_pretty(&full_report) {
            Ok(json) => {
                if let Err(e) = std::fs::write(path, json) {
                    eprintln!("ERROR  failed to write JSON report to {path}: {e}");
                } else {
                    eprintln!("audit-ahb: JSON report written to {path}");
                }
            }
            Err(e) => eprintln!("ERROR  JSON serialisation failed: {e}"),
        }
    }

    let ok = below_density.is_empty() && below_cond.is_empty();
    if !ok {
        std::process::exit(1);
    }
    ok
}

// ── Core analysis ─────────────────────────────────────────────────────────────

/// EDIFACT envelope / control segments that are enforced by the MIG layer,
/// not the AHB layer.  Excluding them from segment-coverage metrics gives a
/// meaningful "AHB rule coverage" figure that only counts application segments.
const EDIFACT_STRUCTURE_SEGS: &[&str] = &["UNH", "UNT", "UNS", "UNB", "UNZ", "UIH", "UIT"];

fn analyse_profile(
    msg_type: &str,
    release_dir: &str,
    mig: &MigProfile,
    ahb: &AhbProfile,
    report_unmodelled: bool,
) -> ProfileReport {
    // Collect ALL MIG segment tags (root + nested in groups), excluding
    // EDIFACT structure segments that are MIG-layer-enforced, not AHB-layer.
    let mut all_mig_tags: BTreeSet<String> = BTreeSet::new();
    for seg in &mig.segments {
        if !EDIFACT_STRUCTURE_SEGS.contains(&seg.tag.as_str()) {
            all_mig_tags.insert(seg.tag.clone());
        }
    }
    for group in &mig.segment_groups {
        collect_group_tags(group, &mut all_mig_tags);
    }

    // Collect mandatory MIG groups.
    let mut mandatory_groups: Vec<(String, String)> = Vec::new(); // (group_id, trigger)
    for group in &mig.segment_groups {
        collect_mandatory_groups(group, &mut mandatory_groups);
    }

    // Segments covered by at least one PID's segment_rules.
    let mut covered_by_any: BTreeSet<String> = BTreeSet::new();
    // Groups that have at least one group_rule in any PID.
    let mut groups_with_rules: BTreeSet<String> = BTreeSet::new();

    let mut total_seg_rules = 0usize;
    let mut total_grp_rules = 0usize;
    let mut total_cond_rules = 0usize;
    let mut unmodelled_seg: Vec<UnmodelledRule> = Vec::new();
    let mut unmodelled_grp: Vec<UnmodelledRule> = Vec::new();

    for pid in &ahb.pruefidentifikatoren {
        for sr in &pid.segment_rules {
            covered_by_any.insert(sr.tag.clone());
            total_seg_rules += 1;
            total_cond_rules += sr.conditional_rules.len();
            if report_unmodelled
                && sr.conditional_rules.is_empty()
                && sr.description.as_deref().is_some_and(|d| !d.is_empty())
            {
                unmodelled_seg.push(UnmodelledRule {
                    pid: pid.code,
                    id: sr.tag.clone(),
                    description: sr.description.clone().unwrap_or_default(),
                });
            }
        }
        for gr in &pid.group_rules {
            groups_with_rules.insert(gr.group_id.clone());
            total_grp_rules += 1;
            total_cond_rules += gr.conditional_rules.len();
            if report_unmodelled
                && gr.conditional_rules.is_empty()
                && gr.description.as_deref().is_some_and(|d| !d.is_empty())
            {
                unmodelled_grp.push(UnmodelledRule {
                    pid: pid.code,
                    id: gr.group_id.clone(),
                    description: gr.description.clone().unwrap_or_default(),
                });
            }
        }
    }

    let pid_count = ahb.pruefidentifikatoren.len();
    let total_rules = total_seg_rules + total_grp_rules;
    let avg_rules_per_pid = if pid_count > 0 {
        total_rules as f64 / pid_count as f64
    } else {
        0.0
    };

    let uncovered: Vec<String> = all_mig_tags
        .iter()
        .filter(|t| !covered_by_any.contains(*t))
        .cloned()
        .collect();
    let covered: Vec<String> = covered_by_any.iter().cloned().collect();

    let mandatory_gaps: Vec<MandatoryGroupGap> = mandatory_groups
        .iter()
        .filter(|(id, _)| !groups_with_rules.contains(id.as_str()))
        .map(|(id, trigger)| MandatoryGroupGap {
            group_id: id.clone(),
            trigger_segment: trigger.clone(),
        })
        .collect();

    ProfileReport {
        message_type: msg_type.to_string(),
        release_dir: release_dir.to_string(),
        pid_count,
        mig_segment_count: all_mig_tags.len(),
        total_segment_rules: total_seg_rules,
        total_group_rules: total_grp_rules,
        total_conditional_rules: total_cond_rules,
        avg_rules_per_pid,
        covered_segments: covered,
        uncovered_segments: uncovered,
        mandatory_groups_without_rules: mandatory_gaps,
        unmodelled_segment_rules: unmodelled_seg,
        unmodelled_group_rules: unmodelled_grp,
        archived: mig.archived || mig.pid_exempt,
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn collect_group_tags(group: &MigGroup, out: &mut BTreeSet<String>) {
    for seg in &group.segments {
        // Exclude EDIFACT structure segments from AHB coverage metrics.
        if !EDIFACT_STRUCTURE_SEGS.contains(&seg.tag.as_str()) {
            out.insert(seg.tag.clone());
        }
    }
    for nested in &group.groups {
        collect_group_tags(nested, out);
    }
}

fn collect_mandatory_groups(group: &MigGroup, out: &mut Vec<(String, String)>) {
    if group.mandatory {
        out.push((group.id.clone(), group.trigger_segment.clone()));
    }
    for nested in &group.groups {
        collect_mandatory_groups(nested, out);
    }
}

fn print_table(
    reports: &[ProfileReport],
    summary: &AuditSummary,
    min_density: f64,
    min_cond_rules: usize,
) {
    // Header
    eprintln!();
    eprintln!(
        "{:<45} {:>4} {:>7} {:>8} {:>8} {:>8} {:>8} {:>6}  Flags",
        "Profile", "PIDs", "MIGsegs", "SegRules", "GrpRules", "CondRls", "Avg/PID", "Cover%",
    );
    eprintln!("{}", "-".repeat(115));

    for r in reports {
        let coverage_pct = if r.mig_segment_count > 0 {
            (r.covered_segments.len() as f64 / r.mig_segment_count as f64) * 100.0
        } else {
            0.0
        };
        let density_flag = if min_density > 0.0
            && r.avg_rules_per_pid < min_density
            && !r.archived
            && r.pid_count > 0
        {
            "LOW-DENSITY"
        } else {
            ""
        };
        let cond_flag = if min_cond_rules > 0
            && r.total_conditional_rules < min_cond_rules
            && !r.archived
            && r.pid_count > 0
        {
            "LOW-COND"
        } else {
            ""
        };
        let archived_flag = if r.archived { "archived/exempt" } else { "" };
        let zero_pid_flag = if r.pid_count == 0 && !r.archived {
            "NO-PIDS"
        } else {
            ""
        };
        let flags: Vec<&str> = [density_flag, cond_flag, archived_flag, zero_pid_flag]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect();

        let key = format!("{}/{}", r.message_type.to_lowercase(), r.release_dir);
        eprintln!(
            "{:<45} {:>4} {:>7} {:>8} {:>8} {:>8} {:>8.1} {:>5.0}%  {}",
            key,
            r.pid_count,
            r.mig_segment_count,
            r.total_segment_rules,
            r.total_group_rules,
            r.total_conditional_rules,
            r.avg_rules_per_pid,
            coverage_pct,
            flags.join(" "),
        );

        // Show uncovered segments and mandatory-group gaps as sub-lines.
        if !r.uncovered_segments.is_empty() {
            let tags = r.uncovered_segments.join(" ");
            eprintln!("  {:>12} uncovered: {tags}", "segs:");
        }
        if !r.mandatory_groups_without_rules.is_empty() {
            let gaps: Vec<String> = r
                .mandatory_groups_without_rules
                .iter()
                .map(|g| format!("{}({})", g.group_id, g.trigger_segment))
                .collect();
            eprintln!("  {:>12} no group_rules: {}", "mand-grps:", gaps.join(" "));
        }
    }

    eprintln!("{}", "-".repeat(115));
    eprintln!(
        "TOTAL  {} profiles  {} PIDs  {} seg_rules  {} grp_rules  {} cond_rules",
        summary.total_profiles,
        summary.total_pids,
        summary.total_segment_rules,
        summary.total_group_rules,
        summary.total_conditional_rules,
    );
    if summary.profiles_with_zero_pids > 0 {
        eprintln!(
            "WARNING  {} profile(s) have 0 PIDs (no AHB Prüfidentifikatoren defined)",
            summary.profiles_with_zero_pids
        );
    }
    if !summary.profiles_below_density_threshold.is_empty() {
        eprintln!(
            "FAIL  {} profile(s) below density threshold {min_density}:  {}",
            summary.profiles_below_density_threshold.len(),
            summary.profiles_below_density_threshold.join(", ")
        );
    }
    if !summary.profiles_below_cond_rules_threshold.is_empty() {
        eprintln!(
            "FAIL  {} profile(s) below conditional-rules threshold {min_cond_rules}:  {}",
            summary.profiles_below_cond_rules_threshold.len(),
            summary.profiles_below_cond_rules_threshold.join(", ")
        );
    }
    eprintln!();
}

fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| {
                let e = e.ok()?;
                if e.path().is_dir() {
                    Some(e.path())
                } else {
                    None
                }
            })
            .collect()
        })
        .unwrap_or_default();
    entries.sort();
    entries
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("read error: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("JSON parse error at {}: {e}", path.display()))
}

fn chrono_now_approx() -> String {
    // Use the UNIX epoch as a stable fallback; real timestamps require a dep.
    // For CI artifacts the file mtime is more authoritative anyway.
    if let Ok(dur) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        let secs = dur.as_secs();
        let year = 1970 + secs / 31_557_600;
        format!("{year}-??-?? (unix={secs})")
    } else {
        "unknown".to_string()
    }
}
