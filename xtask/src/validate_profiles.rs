//! Profile consistency checker for `cargo xtask validate-profiles`.
//!
//! Checks all `profiles/<type>/<release>/` directories for:
//! - JSON Schema conformance of `mig.json`, `ahb.json`, `codelists.json`
//! - Presence of all three JSON files (`mig.json`, `ahb.json`, `codelists.json`)
//! - `release` field inside each JSON matching the directory name
//! - `message_type` field matching the parent directory name
//! - All qualifier-restriction codes in `ahb.json` existing in `codelists.json`
//! - All qualifier-restriction codes in `mig.json` existing in `codelists.json`
//! - PID codes in `ahb.json` being valid 5-digit integers (10000–99999)
//! - Segment tags in `ahb.json` `segment_rules` being a subset of those in `mig.json`
//! - `element_index` in `ahb.json` `field_rules` not exceeding the segment's element count

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ── Minimal JSON models (mirrors codegen.rs but only what's needed) ───────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct MigProfile {
    schema_version: u32,
    message_type: String,
    release: String,
    #[serde(default)]
    ordering_hint: Vec<String>,
    #[serde(default)]
    pid_source: String,
    // Added in  expiry date for release transition modeling.
    #[serde(default)]
    valid_until: Option<String>,
    // Added in  links a newer release to the older one it supersedes.
    #[serde(default)]
    supersedes_directory: Option<String>,
    // Added in  AHB revision identifier.
    #[serde(default)]
    ahb_revision: Option<String>,
    // Added in  source document title.
    #[serde(default)]
    source_document: Option<String>,
    // Added in  explicit archive marker set by `cargo xtask codegen --prune-expired`.
    #[serde(default)]
    archived: bool,
    // Added in  profiles that intentionally have no PIDs (e.g. CONTRL).
    #[serde(default)]
    pid_exempt: bool,
    // Added in  explicit valid_from date matching directory name.
    #[serde(default)]
    valid_from: Option<String>,
    segments: Vec<MigSegment>,
    segment_groups: Vec<MigGroup>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct MigSegment {
    tag: String,
    name: String,
    mandatory: bool,
    max_occurrences: u32,
    elements: Vec<MigElement>,
    #[serde(default)]
    qualifier_restrictions: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct MigElement {
    id: String,
    status: String,
    components: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct MigGroup {
    id: String,
    trigger_segment: String,
    mandatory: bool,
    max_occurrences: u32,
    #[serde(default)]
    min_occurrences: Option<u32>,
    segments: Vec<MigSegment>,
    groups: Vec<MigGroup>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AhbProfile {
    schema_version: u32,
    message_type: String,
    release: String,
    // Added in  expiry date for release transition modeling.
    #[serde(default)]
    valid_until: Option<String>,
    // Added in  links a newer release to the older one it supersedes.
    #[serde(default)]
    supersedes_directory: Option<String>,
    // Added in  AHB revision identifier.
    #[serde(default)]
    ahb_revision: Option<String>,
    // Added in  source document title.
    #[serde(default)]
    source_document: Option<String>,
    /// Human reviewer of this profile before promotion to production.
    ///
    /// Required for `fv2026*` and later profiles to ensure no draft-extracted
    /// profile is shipped without human validation (F-013).
    #[serde(default)]
    reviewed_by: Option<String>,
    /// ISO 8601 date when this profile was reviewed and promoted.
    #[serde(default)]
    reviewed_at: Option<String>,
    pruefidentifikatoren: Vec<PruefidentifikatorEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct PruefidentifikatorEntry {
    code: u32,
    name: String,
    segment_rules: Vec<AhbSegmentRule>,
    /// Per-segment-group-instance rules. Each entry is scoped to a
    /// specific group definition and fires once per group occurrence.
    #[serde(default)]
    group_rules: Vec<AhbGroupRule>,
}

/// A single AHB rule scoped to a segment-group instance.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AhbGroupRule {
    /// Segment-group identifier (e.g. `"SG4"`).
    group_id: String,
    /// EDIFACT segment tag to check within the group (e.g. `"STS"`).
    tag: String,
    /// `M` = mandatory, `N` = must not appear, `O` = optional, `C` = conditional.
    requirement: String,
    /// Qualifier restrictions: element DE identifier → list of allowed values.
    #[serde(default)]
    qualifier_restrictions: BTreeMap<String, Vec<String>>,
    /// BDEW Bedingungsoperator rules scoped to each group occurrence.
    #[serde(default)]
    conditional_rules: Vec<AhbConditionalRule>,
    /// Human-readable description for documentation.
    #[serde(default, rename = "_description")]
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AhbSegmentRule {
    tag: String,
    requirement: String,
    #[serde(default)]
    qualifier_restrictions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    field_rules: Vec<AhbFieldRule>,
    #[serde(default)]
    required_qualifiers: BTreeMap<String, Vec<String>>,
    // Added in  conditional rules for segments with requirement "C".
    #[serde(default)]
    conditional_rules: Vec<AhbConditionalRule>,
    // Added in  human-readable description (not used in validation).
    #[serde(default, rename = "_description")]
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AhbWhenElement {
    element_index: usize,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    value_alternatives: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AhbConditionalRule {
    when_tag: String,
    #[serde(default)]
    when_element_index: usize,
    #[serde(default)]
    when_value: Option<String>,
    #[serde(default)]
    when_value_alternatives: Vec<String>,
    #[serde(default)]
    when_additional_elements: Vec<AhbWhenElement>,
    /// BDEW Bedingungsoperator (I/V/E/X/U/O/G/K/Z).
    #[serde(default)]
    operator: Option<String>,
    /// Secondary trigger segment tag (required for X/U/O operators; optional for K).
    #[serde(default)]
    secondary_tag: Option<String>,
    /// Extra segment tags for the K operator (beyond when_tag and secondary_tag).
    #[serde(default)]
    additional_tags: Vec<String>,
    /// Count threshold for the G (≥N) operator.
    #[serde(default)]
    count_threshold: Option<u32>,
    then_requirement: String,
    #[serde(default)]
    then_qualifier_index: usize,
    #[serde(default)]
    then_qualifier_value: Option<String>,
    #[serde(default, rename = "_description")]
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AhbFieldRule {
    element: String,
    requirement: String,
    #[serde(default)]
    allowed_values: Vec<String>,
    /// 0-based element index within the segment — must be set explicitly.
    element_index: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodelistsData {
    #[serde(default)]
    #[allow(dead_code)]
    schema_version: u32,
    #[allow(dead_code)]
    release: String,
    lists: BTreeMap<String, Vec<String>>,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Compiled JSON Schema validators used during profile checks.
struct Schemas {
    mig: jsonschema::Validator,
    ahb: jsonschema::Validator,
    codelists: jsonschema::Validator,
}

impl Schemas {
    /// Load and compile the three schema files from `schemas_dir`.
    ///
    /// Returns `Err(String)` when a schema file is missing or invalid JSON Schema.
    fn load(schemas_dir: &Path) -> Result<Self, String> {
        let load = |name: &str| -> Result<jsonschema::Validator, String> {
            let path = schemas_dir.join(name);
            let raw = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let schema: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| format!("{}: invalid JSON: {e}", path.display()))?;
            jsonschema::validator_for(&schema)
                .map_err(|e| format!("{}: invalid JSON Schema: {e}", path.display()))
        };
        Ok(Self {
            mig: load("mig.schema.json")?,
            ahb: load("ahb.schema.json")?,
            codelists: load("codelists.schema.json")?,
        })
    }
}

/// Run profile validation from `workspace_root`. Returns `true` if all checks pass.
pub fn run(workspace_root: &str) -> bool {
    let profiles_dir = PathBuf::from(workspace_root)
        .join("crates")
        .join("edi-energy")
        .join("profiles");
    let schemas_dir = profiles_dir.join("schemas");

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut checked = 0u32;

    // Load and compile the JSON Schema validators once, before iterating profiles.
    let schemas = match Schemas::load(&schemas_dir) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!(
                "WARNING  cannot load profile schemas from {}: {e}",
                schemas_dir.display()
            );
            eprintln!("WARNING  JSON Schema validation will be skipped");
            None
        }
    };

    // Track (message_type_upper, wire_release_code) → folder path for duplicate detection.
    // Profiles with `supersedes_directory` intentionally carry the same wire code as the
    // directory they supersede (BDEW correction cycle). These are NOT duplicates.
    let mut seen_wire_releases: BTreeMap<(String, String), String> = BTreeMap::new();

    for msg_type_dir in read_subdirs(&profiles_dir) {
        let dir_name = msg_type_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_uppercase();
        if dir_name == "SCHEMAS" {
            continue;
        }

        for release_dir in read_subdirs(&msg_type_dir) {
            let release = release_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();
            checked += 1;

            let result = check_profile(
                &msg_type_dir,
                &release_dir,
                &dir_name,
                &release,
                schemas.as_ref(),
                &mut errors,
                &mut warnings,
            );

            if let Some((wire, supersedes)) = result {
                let folder = format!("{}/{}", dir_name.to_lowercase(), release);
                if let Some(ref superseded_name) = supersedes {
                    // Correction profile: same wire code as predecessor is intentional.
                    // Verify the directory it claims to supersede actually exists.
                    let superseded_path = msg_type_dir.join(superseded_name);
                    if !superseded_path.is_dir() {
                        errors.push(format!(
                            "{folder}  supersedes_directory {superseded_name:?} does not exist"
                        ));
                    }
                } else {
                    // Originating profile: register as the canonical holder of this wire code.
                    let key = (dir_name.clone(), wire.clone());
                    if let Some(prev) = seen_wire_releases.insert(key, folder.clone()) {
                        errors.push(format!(
                            "{folder}  duplicate wire release code {wire:?} — already registered by {prev}"
                        ));
                    }
                }
            }
        }
    }

    // Print results
    for w in &warnings {
        eprintln!("WARNING  {w}");
    }
    for e in &errors {
        eprintln!("ERROR    {e}");
    }

    let ok = errors.is_empty();
    eprintln!();
    eprintln!(
        "validate-profiles: checked {checked} profile(s) — {} error(s), {} warning(s)",
        errors.len(),
        warnings.len()
    );

    ok
}

/// Convert an `fv<YYYYMMDD>[_<suffix>]` directory name to an ISO 8601 date string.
///
/// Returns `Some("YYYY-MM-DD")` for well-formed FV-date directories, `None` otherwise.
fn parse_fv_date_str(folder_name: &str) -> Option<String> {
    let after_fv = folder_name.strip_prefix("fv")?;
    let digits = after_fv.get(..8)?;
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let rest = &after_fv[8..];
    if !rest.is_empty() && !rest.starts_with('_') {
        return None;
    }
    let year = &digits[0..4];
    let month = &digits[4..6];
    let day = &digits[6..8];
    let mo: u8 = month.parse().ok()?;
    let d: u8 = day.parse().ok()?;
    if mo == 0 || mo > 12 || d == 0 || d > 31 {
        return None;
    }
    Some(format!("{year}-{month}-{day}"))
}

// ── Per-profile checks ────────────────────────────────────────────────────────

/// Check a single profile directory.
///
/// Returns `Some((wire_code, supersedes_directory))` when `mig.json` parsed
/// successfully:
/// - `wire_code` is the `release` JSON field (the BDEW wire association code).
/// - `supersedes_directory` is `Some(name)` when this is a correction profile that
///   intentionally carries the same wire code as the directory it supersedes.
///
/// Returns `None` when the profile is too broken to extract wire identity.
fn check_profile(
    _msg_type_dir: &Path,
    release_dir: &Path,
    expected_type: &str,
    expected_release: &str,
    schemas: Option<&Schemas>,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Option<(String, Option<String>)> {
    let mig_path = release_dir.join("mig.json");
    let ahb_path = release_dir.join("ahb.json");
    let codelists_path = release_dir.join("codelists.json");
    let rel_prefix = format!("{}/{}", expected_type.to_lowercase(), expected_release);

    // 1. Presence check
    if !mig_path.exists() {
        errors.push(format!("{rel_prefix}  missing mig.json"));
    }
    if !ahb_path.exists() {
        errors.push(format!("{rel_prefix}  missing ahb.json"));
    }
    if !codelists_path.exists() {
        errors.push(format!("{rel_prefix}  missing codelists.json"));
    }
    if !mig_path.exists() || !ahb_path.exists() || !codelists_path.exists() {
        return None; // Can't do further checks without all files
    }

    // 2. JSON Schema validation — runs before Rust deserialization so structural
    //    constraints (pattern, enum, required, additionalProperties) are enforced.
    //    This catches malformed release codes, missing required fields, and version
    //    mismatches that serde would silently accept.
    if let Some(schemas) = schemas {
        validate_json_file(
            &mig_path,
            &schemas.mig,
            rel_prefix.as_str(),
            "mig.json",
            errors,
        );
        validate_json_file(
            &ahb_path,
            &schemas.ahb,
            rel_prefix.as_str(),
            "ahb.json",
            errors,
        );
        validate_json_file(
            &codelists_path,
            &schemas.codelists,
            rel_prefix.as_str(),
            "codelists.json",
            errors,
        );
    }

    // 3. Parse JSON into typed structs for deeper cross-file consistency checks.
    let mig = match load_json::<MigProfile>(&mig_path) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!("{rel_prefix}/mig.json  JSON parse error: {e}"));
            return None;
        }
    };
    let ahb = match load_json::<AhbProfile>(&ahb_path) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!("{rel_prefix}/ahb.json  JSON parse error: {e}"));
            return None;
        }
    };
    let codelists = match load_json::<CodelistsData>(&codelists_path) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!(
                "{rel_prefix}/codelists.json  JSON parse error: {e}"
            ));
            return None;
        }
    };

    // 3. Field consistency: release field must be consistent across all three files.
    // For FV-date directories (`fv<YYYYMMDD>`), the folder name encodes the valid_from
    // date while the JSON `release` field holds the wire assoc_code (e.g. "2.4c").
    // In that case we skip the folder-name vs release check but still require all
    // three JSON files to agree on the release value.
    let is_fv_dir = expected_release.starts_with("fv");

    // F-013: For FV2026-10-01 and later profiles, require human review provenance.
    // Profiles promoted from an auto-generated draft without review risk incorrect
    // segment-group validation against the AHB. We require `reviewed_by` in
    // ahb.json for all `fv2026*` (and later) profiles.
    //
    // FV2025 profiles are grandfathered. FV2026+ must have been reviewed.
    if is_fv_dir {
        let after_2025 = expected_release
            .trim_start_matches("fv")
            .split('_')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|v| v > 20_260_101) // any date after 2026-01-01
            .unwrap_or(false);
        if after_2025 && ahb.reviewed_by.is_none() {
            warnings.push(format!(
                "{rel_prefix}/ahb.json  missing `reviewed_by` field — \
                 FV2026+ profiles must be reviewed before production use. \
                 Add {{\"reviewed_by\": \"<reviewer>\", \"reviewed_at\": \"<ISO-date>\"}} \
                 to confirm a human validated this profile against the BDEW AHB PDF. \
                 See FINDINGS.md F-013."
            ));
        }
    }

    if !is_fv_dir {
        if mig.release != expected_release {
            errors.push(format!(
                "{rel_prefix}/mig.json  `release` field {:?} does not match directory {:?}",
                mig.release, expected_release
            ));
        }
        if ahb.release != expected_release {
            errors.push(format!(
                "{rel_prefix}/ahb.json  `release` field {:?} does not match directory {:?}",
                ahb.release, expected_release
            ));
        }
        if codelists.release != expected_release {
            errors.push(format!(
                "{rel_prefix}/codelists.json  `release` field {:?} does not match directory {:?}",
                codelists.release, expected_release
            ));
        }
    }
    // All three files must agree on the release value regardless of directory naming.
    if mig.release != ahb.release {
        errors.push(format!(
            "{rel_prefix}  mig.json release {:?} does not match ahb.json release {:?}",
            mig.release, ahb.release
        ));
    }
    if mig.release != codelists.release {
        errors.push(format!(
            "{rel_prefix}  mig.json release {:?} does not match codelists.json release {:?}",
            mig.release, codelists.release
        ));
    }

    // 4. Field consistency: message_type matches directory (case-insensitive)
    if mig.message_type.to_uppercase() != expected_type {
        errors.push(format!(
            "{rel_prefix}/mig.json  `message_type` field {:?} does not match directory {:?}",
            mig.message_type, expected_type
        ));
    }
    if ahb.message_type.to_uppercase() != expected_type {
        errors.push(format!(
            "{rel_prefix}/ahb.json  `message_type` field {:?} does not match directory {:?}",
            ahb.message_type, expected_type
        ));
    }

    // 4b.  valid_from field must be present and consistent with the directory name.
    if is_fv_dir {
        let expected_date = parse_fv_date_str(expected_release);
        match &mig.valid_from {
            None => errors.push(format!(
                "{rel_prefix}/mig.json  `valid_from` field is missing — add \
                 e.g. {:?} to match the directory name",
                expected_date.as_deref().unwrap_or("YYYY-MM-DD"),
            )),
            Some(vf) if expected_date.as_deref() != Some(vf.as_str()) => errors.push(format!(
                "{rel_prefix}/mig.json  `valid_from` {:?} does not match the \
                 date implied by directory {:?} (expected {:?})",
                vf,
                expected_release,
                expected_date.as_deref().unwrap_or("?"),
            )),
            _ => {}
        }
    }

    // 5. Audit trail: ahb_revision and source_document must be set.
    if mig.ahb_revision.as_deref().unwrap_or("").trim().is_empty() {
        warnings.push(format!(
            "{rel_prefix}/mig.json  `ahb_revision` is missing — add BDEW AHB revision letter for audit traceability"
        ));
    }
    if mig
        .source_document
        .as_deref()
        .unwrap_or("")
        .trim()
        .is_empty()
    {
        warnings.push(format!(
            "{rel_prefix}/mig.json  `source_document` is missing — add BDEW document title (e.g. \"MSCONS AHB 2.4c, Stand 01.10.2025\")"
        ));
    }

    // 6. Build known segment tag set from MIG + segment element-count map
    let mig_tags: BTreeSet<&str> = collect_all_tags(&mig);
    // Map from tag → element count for the first definition in MIG.
    let mig_element_counts: BTreeMap<&str, usize> = collect_segment_element_counts(&mig);

    // 7. Build known code-list keys
    let codelist_keys: BTreeSet<&str> = codelists
        .lists
        .keys()
        .map(std::string::String::as_str)
        .collect();

    // 7. MIG qualifier_restrictions: all referenced DE IDs should be in codelists
    check_mig_qualifiers(&mig, &codelist_keys, &rel_prefix, warnings);

    // 8. AHB checks
    for pid in &ahb.pruefidentifikatoren {
        // PID range check
        if pid.code < 10000 || pid.code > 99999 {
            errors.push(format!(
                "{rel_prefix}/ahb.json  PID {}: code must be in range 10000–99999",
                pid.code
            ));
        }

        // Zero-rule guard: a PID with no segment_rules AND no
        // group_rules is a silent validation hole — every inbound message passes
        // AHB validation vacuously.  Profiles that intentionally have no PIDs set
        // `pid_exempt = true` on their MIG; individual PIDs cannot opt out.
        if pid.segment_rules.is_empty() && pid.group_rules.is_empty() {
            errors.push(format!(
                "{rel_prefix}/ahb.json  PID {}: has zero segment_rules and zero \
                 group_rules — messages will pass AHB validation vacuously. \
                 Import the rules from the official BDEW AHB or mark the profile \
                 `pid_exempt = true` if PIDs are intentionally absent.",
                pid.code
            ));
        }

        for rule in &pid.segment_rules {
            // Segment tag cross-reference
            if !mig_tags.contains(rule.tag.as_str()) {
                warnings.push(format!(
                    "{rel_prefix}/ahb.json  PID {}: segment {:?} not defined in mig.json",
                    pid.code, rule.tag
                ));
            }

            // Requirement value check — M/C/N/O/S/X are valid
            if !matches!(rule.requirement.as_str(), "M" | "C" | "N" | "O" | "S" | "X") {
                errors.push(format!(
                    "{rel_prefix}/ahb.json  PID {}: segment {:?} has invalid requirement {:?} (expected M/C/N/O/S/X)",
                    pid.code, rule.tag, rule.requirement
                ));
            }

            // qualifier_restrictions: DE IDs in codelists
            for de_id in rule.qualifier_restrictions.keys() {
                if !codelist_keys.contains(de_id.as_str()) {
                    warnings.push(format!(
                        "{rel_prefix}/ahb.json  PID {}: segment {:?} qualifier_restriction DE {:?} not in codelists.json",
                        pid.code, rule.tag, de_id
                    ));
                }
            }

            //  conditional rule when_tag and secondary_tag cross-check.
            // Both must refer to segments defined in mig.json.
            for (ci, cond) in rule.conditional_rules.iter().enumerate() {
                if !mig_tags.contains(cond.when_tag.as_str()) {
                    warnings.push(format!(
                        "{rel_prefix}/ahb.json  PID {}: segment {:?} conditional_rule[{ci}] \
                         when_tag {:?} is not defined in mig.json",
                        pid.code, rule.tag, cond.when_tag
                    ));
                }
                if let Some(ref st) = cond.secondary_tag
                    && !mig_tags.contains(st.as_str())
                {
                    warnings.push(format!(
                        "{rel_prefix}/ahb.json  PID {}: segment {:?} conditional_rule[{ci}] \
                             secondary_tag {:?} is not defined in mig.json",
                        pid.code, rule.tag, st
                    ));
                }
                // Operator semantic constraints: X/U/O operators require secondary_tag.
                // K operator requires at least two tags total (when_tag + secondary_tag or additional_tags).
                if let Some(ref op) = cond.operator {
                    if matches!(op.as_str(), "X" | "U" | "O") && cond.secondary_tag.is_none() {
                        warnings.push(format!(
                            "{rel_prefix}/ahb.json  PID {}: segment {:?} conditional_rule[{ci}] \
                             operator {:?} requires secondary_tag to be set",
                            pid.code, rule.tag, op
                        ));
                    }
                    if op == "K" {
                        let total_tags =
                            1 + cond.secondary_tag.is_some() as usize + cond.additional_tags.len();
                        if total_tags < 2 {
                            warnings.push(format!(
                                "{rel_prefix}/ahb.json  PID {}: segment {:?} conditional_rule[{ci}] \
                                 operator K requires at least 2 tags (when_tag + secondary_tag or additional_tags)",
                                pid.code, rule.tag
                            ));
                        }
                        // Validate additional_tags exist in mig.json
                        for at in &cond.additional_tags {
                            if !mig_tags.contains(at.as_str()) {
                                warnings.push(format!(
                                    "{rel_prefix}/ahb.json  PID {}: segment {:?} conditional_rule[{ci}] \
                                     K additional_tag {:?} is not defined in mig.json",
                                    pid.code, rule.tag, at
                                ));
                            }
                        }
                    }
                }
            }

            //  element_index cross-check — warn when index exceeds segment's element count.
            if let Some(&elem_count) = mig_element_counts.get(rule.tag.as_str()) {
                for fr in &rule.field_rules {
                    if fr.element_index >= elem_count {
                        errors.push(format!(
                            "{rel_prefix}/ahb.json  PID {}: segment {:?} field_rule for DE {:?} has element_index {} but segment only has {} element(s)",
                            pid.code, rule.tag, fr.element, fr.element_index, elem_count
                        ));
                    }
                }
            }
        }
    }

    // valid_until >= valid_from (if valid_until is set and we can infer valid_from)
    // For `fv<YYYYMMDD>` directories, parse the valid_from date from the folder name.
    // Dates compared lexicographically as ISO-8601 strings (YYYY-MM-DD format).
    if let Some(valid_until) = &mig.valid_until
        && let Some(valid_from_str) = parse_fv_date(expected_release)
        && valid_until.as_str() < valid_from_str.as_str()
    {
        errors.push(format!(
                "{rel_prefix}/mig.json  valid_until {valid_until:?} is before inferred valid_from {valid_from_str:?}"
            ));
    }

    //  Warn when a non-archived profile has no valid_until and has been open-ended
    // for more than 14 months (> the maximum BDEW annual update cycle).  This surfaces
    // profiles where a successor should have been authored but wasn't.
    if !mig.archived
        && mig.valid_until.is_none()
        && let Some(valid_from_str) = parse_fv_date(expected_release)
    {
        // Compute age in months by comparing YYYY-MM prefixes (lexicographic is safe for ISO-8601).
        // "2024-10" + 14 months = "2026-00" — use integer arithmetic instead.
        if let (Ok(year), Ok(month)) = (
            valid_from_str[..4].parse::<u32>(),
            valid_from_str[5..7].parse::<u32>(),
        ) {
            let expire_year = year + (month + 13) / 12;
            let expire_month = (month + 13) % 12 + 1; // 14 months after valid_from
            let cutoff = format!("{expire_year:04}-{expire_month:02}");
            // Use today's date (YYYY-MM) to determine if this profile is stale.
            // Parse today's date from RFC3339 timestamp to avoid adding the `time` crate.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            // Approximate: seconds since epoch → YYYY-MM (good enough for 14-month check).
            // 1970-01-01 + n_days → year/month via div/mod. Use simple epoch arithmetic.
            let days = now / 86400;
            // Gregorian calendar epoch arithmetic (Zeller-style, no leap-second issues for months).
            let (today_year, today_month) = days_to_year_month(days);
            let today_ym = format!("{today_year:04}-{today_month:02}");
            if today_ym >= cutoff {
                warnings.push(format!(
                    "{rel_prefix}/mig.json  no `valid_until` set — profile has been open-ended for \
                     >14 months (valid_from {valid_from_str:?}). Set `valid_until` when BDEW publishes \
                     the successor, or set `pid_exempt: true` if this type has no annual update"
                ));
            }
        }
    }

    // schema_version must be >= SCHEMA_MIN
    const SCHEMA_MIN: u32 = 1;
    if mig.schema_version < SCHEMA_MIN {
        errors.push(format!(
            "{rel_prefix}/mig.json  schema_version {} is below minimum {SCHEMA_MIN}",
            mig.schema_version
        ));
    }
    if ahb.schema_version < SCHEMA_MIN {
        errors.push(format!(
            "{rel_prefix}/ahb.json  schema_version {} is below minimum {SCHEMA_MIN}",
            ahb.schema_version
        ));
    }

    // Implausible segment counts — fewer than 3 top-level segments is suspicious.
    let total_seg_count = count_all_segments(&mig);
    if total_seg_count < 3 {
        warnings.push(format!(
            "{rel_prefix}/mig.json  only {total_seg_count} segment(s) defined — possible empty or copy-paste error"
        ));
    }

    // max_occurrences = 0 on any segment is always wrong.
    for seg in &mig.segments {
        if seg.max_occurrences == 0 {
            errors.push(format!(
                "{rel_prefix}/mig.json  segment {:?} has max_occurrences = 0 (must be >= 1)",
                seg.tag
            ));
        }
    }

    //  AHB rule-coverage quality gates.
    // These are WARNING-level; they do not block CI but make gaps visible.
    if !mig.archived && !mig.pid_exempt {
        let pid_count = ahb.pruefidentifikatoren.len();
        if pid_count == 0 {
            warnings.push(format!(
                "{rel_prefix}/ahb.json  no Prüfidentifikatoren defined — set `pid_exempt: true` in mig.json if this is intentional (e.g. CONTRL)"
            ));
        } else {
            // Avg rules / PID density check.
            // Only meaningful when the profile has enough non-EDIFACT segments to
            // reach the threshold; tiny profiles (e.g. ORDCHG with 4 app segments)
            // are exempt.
            let total_rules: usize = ahb
                .pruefidentifikatoren
                .iter()
                .map(|p| p.segment_rules.len() + p.group_rules.len())
                .sum();
            let avg = total_rules as f64 / pid_count as f64;
            const MIN_DENSITY: f64 = 5.0;
            const EDIFACT_STRUCTURE: &[&str] = &["UNH", "UNT", "UNS", "UNB", "UNZ", "UIH", "UIT"];
            let app_seg_count = collect_all_tags(&mig)
                .iter()
                .filter(|t| !EDIFACT_STRUCTURE.contains(t))
                .count();
            if avg < MIN_DENSITY && app_seg_count as f64 >= MIN_DENSITY {
                warnings.push(format!(
                    "{rel_prefix}/ahb.json  avg rules/PID is {avg:.1} (threshold {MIN_DENSITY}) — AHB coverage is critically sparse"
                ));
            }

            // Mandatory-group coverage: every mandatory MIG group should have at
            // least one group_rule in at least one PID.
            let groups_with_rules: BTreeSet<&str> = ahb
                .pruefidentifikatoren
                .iter()
                .flat_map(|p| p.group_rules.iter().map(|gr| gr.group_id.as_str()))
                .collect();
            let mandatory_uncovered =
                collect_mandatory_group_gaps(&mig.segment_groups, &groups_with_rules);
            for (group_id, trigger) in &mandatory_uncovered {
                warnings.push(format!(
                    "{rel_prefix}/ahb.json  mandatory group {group_id:?} (trigger: {trigger:?}) has no group_rule in any PID — add per-instance trigger-segment enforcement"
                ));
            }
        }
    }

    Some((mig.release.clone(), mig.supersedes_directory.clone()))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn collect_all_tags(mig: &MigProfile) -> BTreeSet<&str> {
    let mut tags = BTreeSet::new();
    for seg in &mig.segments {
        tags.insert(seg.tag.as_str());
    }
    for group in &mig.segment_groups {
        collect_group_tags(group, &mut tags);
    }
    tags
}

/// Build a map from segment tag → number of elements, using the first definition found.
fn collect_segment_element_counts(mig: &MigProfile) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for seg in &mig.segments {
        counts.entry(seg.tag.as_str()).or_insert(seg.elements.len());
    }
    for group in &mig.segment_groups {
        collect_group_element_counts(group, &mut counts);
    }
    counts
}

fn collect_group_element_counts<'a>(group: &'a MigGroup, counts: &mut BTreeMap<&'a str, usize>) {
    for seg in &group.segments {
        counts.entry(seg.tag.as_str()).or_insert(seg.elements.len());
    }
    for nested in &group.groups {
        collect_group_element_counts(nested, counts);
    }
}

fn collect_group_tags<'a>(group: &'a MigGroup, tags: &mut BTreeSet<&'a str>) {
    for seg in &group.segments {
        tags.insert(seg.tag.as_str());
    }
    for nested in &group.groups {
        collect_group_tags(nested, tags);
    }
}

fn check_mig_qualifiers(
    mig: &MigProfile,
    codelist_keys: &BTreeSet<&str>,
    rel_prefix: &str,
    warnings: &mut Vec<String>,
) {
    for seg in &mig.segments {
        for de_id in seg.qualifier_restrictions.keys() {
            if !codelist_keys.contains(de_id.as_str()) {
                warnings.push(format!(
                    "{rel_prefix}/mig.json  segment {:?} qualifier_restriction DE {:?} not in codelists.json",
                    seg.tag, de_id
                ));
            }
        }
    }
    for group in &mig.segment_groups {
        check_group_qualifiers(group, codelist_keys, rel_prefix, warnings);
    }
}

fn check_group_qualifiers(
    group: &MigGroup,
    codelist_keys: &BTreeSet<&str>,
    rel_prefix: &str,
    warnings: &mut Vec<String>,
) {
    for seg in &group.segments {
        for de_id in seg.qualifier_restrictions.keys() {
            if !codelist_keys.contains(de_id.as_str()) {
                warnings.push(format!(
                    "{rel_prefix}/mig.json  group {:?} segment {:?} qualifier_restriction DE {:?} not in codelists.json",
                    group.id, seg.tag, de_id
                ));
            }
        }
    }
    for nested in &group.groups {
        check_group_qualifiers(nested, codelist_keys, rel_prefix, warnings);
    }
}

fn read_subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// Validate a raw JSON file against a compiled `jsonschema::Validator`.
///
/// All schema violations are appended to `errors` as:
/// `"<rel_prefix>/<filename>  schema: <message> at <instance_path>"`
///
/// Non-fatal I/O or JSON-parse errors are also pushed to `errors` (they prevent
/// meaningful schema validation).
fn validate_json_file(
    path: &Path,
    validator: &jsonschema::Validator,
    rel_prefix: &str,
    filename: &str,
    errors: &mut Vec<String>,
) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!(
                "{rel_prefix}/{filename}  cannot read for schema validation: {e}"
            ));
            return;
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            errors.push(format!(
                "{rel_prefix}/{filename}  invalid JSON (cannot validate against schema): {e}"
            ));
            return;
        }
    };
    for error in validator.iter_errors(&value) {
        errors.push(format!(
            "{rel_prefix}/{filename}  schema: {} at {}",
            error,
            error.instance_path()
        ));
    }
}

/// Count the total number of segments (top-level and within groups) in a MIG profile.
fn count_all_segments(mig: &MigProfile) -> usize {
    let mut count = mig.segments.len();
    for group in &mig.segment_groups {
        count += count_group_segments(group);
    }
    count
}

fn count_group_segments(group: &MigGroup) -> usize {
    let mut count = group.segments.len();
    for nested in &group.groups {
        count += count_group_segments(nested);
    }
    count
}

/// Collect mandatory groups that have no group_rule in any PID.
/// Returns `Vec<(group_id, trigger_segment)>` sorted by group_id.
fn collect_mandatory_group_gaps<'a>(
    groups: &'a [MigGroup],
    groups_with_rules: &BTreeSet<&str>,
) -> Vec<(&'a str, &'a str)> {
    let mut gaps: Vec<(&str, &str)> = Vec::new();
    for g in groups {
        // Only warn when the mandatory group has non-trigger mandatory segments that
        // are not covered by any group_rule.  A trigger segment is implicitly present
        // whenever the group itself exists, so a group_rule for the trigger is
        // redundant and must not be required here.
        let has_non_trigger_mandatory = g
            .segments
            .iter()
            .any(|s| s.mandatory && s.tag != g.trigger_segment);
        if g.mandatory && has_non_trigger_mandatory && !groups_with_rules.contains(g.id.as_str()) {
            gaps.push((g.id.as_str(), g.trigger_segment.as_str()));
        }
        let nested = collect_mandatory_group_gaps(&g.groups, groups_with_rules);
        gaps.extend(nested);
    }
    gaps
}

/// Parse a folder name of the form `fv<YYYYMMDD>` into an ISO-8601 date string `YYYY-MM-DD`.
/// Returns `None` if the folder name does not match.
fn parse_fv_date(folder_name: &str) -> Option<String> {
    let digits = folder_name.strip_prefix("fv")?;
    if digits.len() != 8 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (year, rest) = digits.split_at(4);
    let (month, day) = rest.split_at(2);
    Some(format!("{year}-{month}-{day}"))
}

/// Convert a Unix day count (days since 1970-01-01) to `(year, month)`.
///
/// Uses the proleptic Gregorian calendar — accurate for any date after 1970.
/// Sufficient precision for the 14-month open-ended profile check.
fn days_to_year_month(days: u64) -> (u32, u32) {
    // Algorithm: civil_from_days (Howard Hinnant, public domain).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u32; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // year of era [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month index in [0, 11] starting from March
    let m: i64 = mp as i64 + if mp < 10 { 3 } else { -9i64 }; // month [1, 12]
    let year = y + if m <= 2 { 1 } else { 0 };
    (year as u32, m as u32)
}
