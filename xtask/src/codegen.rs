//! Code-generation logic for `crates/edi-energy/src/generated/`.
//!
//! Reads all `profiles/<type>/<release>/{mig,ahb,codelists}.json` files
//! from the workspace and emits one Rust source file per `(type, release)` pair
//! plus an updated `mod.rs` that declares all sub-modules and wires
//! `register_profiles`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

// ── JSON model ────────────────────────────────────────────────────────────────

/// Deserialisation-only mirror of `PidSource` in the runtime crate.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PidSourceJson {
    /// Prüfidentifikator is in BGM element 1 (DE 1004). Default for all message types.
    #[default]
    BgmDe1004,
    /// Prüfidentifikator is in the first top-level RFF segment with qualifier Z13.
    RffZ13,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigProfile {
    schema_version: u32,
    message_type: String,
    release: String,
    segments: Vec<MigSegment>,
    segment_groups: Vec<MigGroup>,
    /// Explicit segment-tag ordering for the generated `rule_segment_order`.
    ///
    /// When present this list is used verbatim as `EXPECTED_ORDER`.  When absent
    /// the codegen derives the order automatically from `segments` then
    /// `segment_groups`.  Use this for multi-section messages (e.g. MSCONS) where
    /// a section-control segment (UNS) appears between header and detail groups —
    /// the automatic derivation always places top-level segments before groups,
    /// which produces the wrong order for such messages.
    #[serde(default)]
    ordering_hint: Vec<String>,
    /// Where the Prüfidentifikator is located in this message type.
    ///
    /// `"bgm_de1004"` (default): extracted from BGM element 1 (DE 1004).
    /// `"rff_z13"`: extracted from the first top-level RFF segment with qualifier Z13.
    #[serde(default)]
    pid_source: PidSourceJson,
    /// First date on which this profile is normatively valid (BDEW 'Gültig ab').
    /// ISO 8601 date string e.g. "2025-10-01".  Should match the `fvYYYYMMDD`
    /// component of the profile directory name.  When absent the codegen falls
    /// back to parsing the date from the directory name for backward
    /// compatibility; new profiles must set this field explicitly.
    #[serde(default)]
    valid_from: Option<String>,
    /// Last date on which this profile is normatively valid (BDEW 'Gültig bis').
    /// ISO 8601 date string e.g. "2026-09-30". Absent means open-ended.
    #[serde(default)]
    valid_until: Option<String>,
    /// AHB revision letter for same-wire-code correction tracking.
    /// May differ from the wire release code, e.g. "3.2" for MIG release "2.4c".
    #[serde(default)]
    ahb_revision: Option<String>,
    /// BDEW document title this profile was derived from, including revision date.
    #[serde(default)]
    source_document: Option<String>,
    /// Directory name of the earlier same-wire-code profile this supersedes.
    /// Required when two directories share the same wire release code.
    #[serde(default)]
    supersedes_directory: Option<String>,
    /// When `true`, this profile is compiled only when the `{message_type}-archive`
    /// or `archive` Cargo feature is enabled.  Set by `cargo xtask codegen
    /// --prune-expired` when the profile's `valid_until` date has passed the
    /// configured grace period.  Defaults to `false` (active profile).
    ///
    /// Using an explicit JSON field (rather than recomputing from `valid_until`
    /// at codegen time) makes the generated `mod.rs` reproducible regardless of
    /// when `cargo xtask codegen` is run — a required property for the `--check`
    /// CI drift guard.
    #[serde(default)]
    archived: bool,
    /// When `true`, this profile intentionally has no Prüfidentifikatoren and
    /// AHB rule-coverage warnings are suppressed.  Use for message types whose
    /// BDEW AHB does not assign business-level PIDs (e.g. CONTRL).
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "read from JSON for validation; not used in code generation itself"
    )]
    pid_exempt: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigSegment {
    tag: String,
    name: String,
    mandatory: bool,
    max_occurrences: u32,
    elements: Vec<MigElement>,
    /// Qualifier values allowed for this segment position in the MIG structural layer.
    /// Accepted from JSON schema for future structural validation; the AHB layer
    /// handles qualifier enforcement today via `AhbSegmentRule.qualifier_restrictions`.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "reserved for future MIG-layer structural qualifier validation; AHB layer enforces qualifiers today"
    )]
    qualifier_restrictions: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigElement {
    id: String,
    status: String,
    components: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MigGroup {
    id: String,
    trigger_segment: String,
    mandatory: bool,
    max_occurrences: u32,
    /// Minimum number of times this group must appear.
    ///
    /// When absent, defaults to `1` for mandatory groups and `0` for optional groups.
    /// A mandatory group with no explicit `min_occurrences` implies exactly-1-or-more.
    #[serde(default)]
    min_occurrences: Option<u32>,
    segments: Vec<MigSegment>,
    groups: Vec<MigGroup>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AhbProfile {
    schema_version: u32,
    /// Wire-format message type code. Validated to match `mig.json` but not re-read in emit
    /// (the codegen receives `message_type` as a function argument from the MIG profile).
    #[expect(
        dead_code,
        reason = "validated against MIG message_type at parse time; codegen uses the MIG-derived value"
    )]
    message_type: String,
    release: String,
    /// BDEW document title. Stored for traceability; not emitted into generated code.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "traceability metadata; emitted into ProfileData which reads from MIG profile"
    )]
    source_document: Option<String>,
    /// Validity end date. Stored for cross-check; `ProfileData.valid_until` is read from MIG.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "validity metadata; codegen reads valid_until from MIG profile, not AHB"
    )]
    valid_until: Option<String>,
    /// AHB revision letter (e.g. "3.2e"). Stored for traceability; emitted via MIG profile.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "traceability metadata; emitted via MIG profile's ahb_revision field"
    )]
    ahb_revision: Option<String>,
    /// Earlier profile this supersedes. Stored for cross-check; read from MIG profile.
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "supersession metadata; codegen reads supersedes_directory from MIG profile"
    )]
    supersedes_directory: Option<String>,
    pruefidentifikatoren: Vec<PruefidentifikatorEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PruefidentifikatorEntry {
    code: u32,
    /// Human-readable name for this Prüfidentifikator (e.g. "Lieferschein").
    /// Stored for traceability; the generated rule functions use only `code`.
    #[expect(
        dead_code,
        reason = "documentation label; only `code` is used in generated rule function names and IDs"
    )]
    name: String,
    segment_rules: Vec<AhbSegmentRule>,
    /// Per-segment-group-instance rules. Each entry scopes a rule to a specific
    /// group definition (e.g. `"SG4"`) and fires once per group occurrence.
    /// This is the AHB-layer counterpart of the MIG group cardinality rules.
    #[serde(default)]
    group_rules: Vec<AhbGroupRule>,
}

/// A single AHB rule scoped to a segment-group instance.
///
/// Unlike `AhbSegmentRule` (which evaluates over the flat message segment list),
/// `AhbGroupRule` is evaluated independently for each occurrence of `group_id`.
/// This lets the AHB encoder express "every SG4 instance must contain an STS
/// segment" rather than "the message must contain at least one STS somewhere".
///
/// `conditional_rules` extends the group-rule schema to support BDEW
/// Bedingungsoperator conditions scoped to individual group instances.
/// For example: "within each SG10 occurrence, if QTY+67 is present, then
/// STS+Z32 must also be present in that same SG10 occurrence."
///
/// This directly resolves F-001: the `segs` parameter passed to the generated
/// `with_scoped_group_rule_fn` closure is already the per-instance sub-slice
/// (edifact-rs `walk_group_tree` slices via `total_span` before calling the
/// closure), so no further scoping is needed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AhbGroupRule {
    /// Segment-group identifier this rule is scoped to (e.g. `"SG4"`).
    group_id: String,
    /// EDIFACT segment tag to check within the group (e.g. `"STS"`).
    tag: String,
    /// `M` = mandatory in every occurrence, `N` = must not appear, `O` = optional
    /// (used only with `qualifier_restrictions` to enforce qualifier values).
    requirement: String,
    /// Qualifier restrictions keyed by EDIFACT DE identifier string.
    /// Maps to the list of allowed qualifier values for that element (element 0,
    /// component 0 by default, matching `ahb_check_qualifier` semantics).
    #[serde(default)]
    qualifier_restrictions: BTreeMap<String, Vec<String>>,
    /// BDEW Bedingungsoperator rules scoped to each group instance.
    ///
    /// Each rule is evaluated against `segs`, which is already the per-instance
    /// sub-slice within the `with_scoped_group_rule_fn` closure.  This is the
    /// primary mechanism for F-001 (intra-SG conditional rule evaluation).
    ///
    /// Example: within each SG10, "if QTY+67 → STS+Z32 must be present".
    #[serde(default)]
    conditional_rules: Vec<AhbConditionalRule>,
    /// Human-readable description stored only for documentation purposes.
    /// Not emitted; intended for JSON authoring context.
    #[serde(default, rename = "_description")]
    #[expect(
        dead_code,
        reason = "JSON authoring aid; `AhbConditionalRule.description` is emitted, but group-rule description is not"
    )]
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AhbSegmentRule {
    tag: String,
    requirement: String,
    #[serde(default)]
    qualifier_restrictions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    field_rules: Vec<AhbFieldRule>,
    /// Require that at least one occurrence of this segment has the specified
    /// qualifier value for the given DE. For example `{"2005": ["137"]}` means
    /// "at least one DTM segment must have DE 2005 = '137'".
    #[serde(default)]
    required_qualifiers: BTreeMap<String, Vec<String>>,
    /// BDEW Bedingungsoperator rules.  Used when `requirement == "C"` (conditional).
    /// Each rule specifies a trigger condition on another segment and what requirement
    /// applies to this segment when the condition holds.
    #[serde(default)]
    conditional_rules: Vec<AhbConditionalRule>,
    /// Human-readable note (origin, BDEW condition number). Not used in code gen.
    #[serde(default, rename = "_description")]
    #[expect(dead_code, reason = "documentation metadata only")]
    description: String,
}

/// BDEW Bedingungsoperator — defines the logical relationship between two or
/// more segment/field references in an AHB conditional rule.
///
/// BDEW AHB Kapitel 6 defines nine categories.  All nine are now representable
/// in the JSON schema via the `operator` field on `AhbConditionalRule`.
#[derive(Debug, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
enum AhbOperator {
    /// (I) If trigger segment A is present, then target segment B must appear.
    ///
    /// Rendered as:
    ///   `if A_present { require B }`
    #[default]
    I,
    /// (V) If trigger segment A is absent, then target segment B must appear.
    ///
    /// Rendered as:
    ///   `if !A_present { require B }`
    V,
    /// (E) If trigger segment A is present, then target segment B must NOT appear.
    ///
    /// Rendered as:
    ///   `if A_present { forbid B }`
    E,
    /// (X) Exactly one of {A, B} must appear (exclusive OR).
    ///
    /// Requires `secondary_tag` to identify segment B.
    /// Rendered as:
    ///   `let a = A_present; let b = B_present;`
    ///   `if !(a ^ b) { error("exactly one of A, B must appear") }`
    X,
    /// (U) Both A and B must appear (AND conjunction).
    ///
    /// Requires `secondary_tag` to identify segment B.
    /// Rendered as:
    ///   `if !A_present || !B_present { error("both A and B are required") }`
    U,
    /// (O) At least one of {A, B} must appear (OR conjunction).
    ///
    /// Requires `secondary_tag` to identify segment B.
    /// Rendered as:
    ///   `if !A_present && !B_present { error("at least one of A or B must appear") }`
    O,
    /// (G) If the trigger segment appears more than `count_threshold` times,
    /// then target segment B must appear.
    ///
    /// Rendered as:
    ///   `let count = segments.iter().filter(|s| s.tag == A).count();`
    ///   `if count > N { require B }`
    G,
    /// (K) At most one of the listed segments may appear (weak mutual exclusion).
    ///
    /// Operands are `when_tag`, optionally `secondary_tag`, and any entries in
    /// `additional_tags`.  Zero or one may be present; two or more is an error.
    ///
    /// Rendered as:
    ///   `let __present = [A, B, ...].iter().filter(|&p| *p).count();`
    ///   `if __present > 1 { error("at most one of {...} may appear") }`
    K,
    /// (Z) Mutually exclusive qualifier-gated rules.  When segment A appears
    /// with qualifier Q1 then B is required; when A appears with Q2 then B
    /// must not appear.  Encode as two separate `E`/`I` rules with
    /// `when_value` set.  This variant is kept for explicit documentation;
    /// the codegen treats it identically to `I`/`E` (the qualifier filtering
    /// is applied via `when_value`/`when_element_index` on each rule).
    Z,
}

/// A BDEW Bedingungsoperator conditional rule.
///
/// Encodes patterns like:
///   - `Muss [92]` where `[92] = "wenn QTY+6063=67 vorhanden"`:
///     → `operator="I", when_tag="QTY", when_element_index=0, when_value="67", then_requirement="M"`
///   - `Nicht zu verwenden [X]` where `[X] = "wenn Segment Y vorhanden"`:
///     → `operator="E", when_tag="Y", then_requirement="N"`
///   - `Muss wenn RFF+AGK nicht vorhanden`:
///     → `operator="V", when_tag="RFF", when_value="AGK", then_requirement="M"`
///   - `Genau eines von {STS, CTA} muss erscheinen`:
///     → `operator="X", when_tag="STS", secondary_tag="CTA"`
///   - `Beide STS und CTA müssen erscheinen`:
///     → `operator="U", when_tag="STS", secondary_tag="CTA"`
///   - `Mindestens eines von {STS, CTA} muss erscheinen`:
///     → `operator="O", when_tag="STS", secondary_tag="CTA"`
///   - `Wenn QTY mehr als 2 mal vorhanden, dann DTM Pflicht`:
///     → `operator="G", when_tag="QTY", count_threshold=2, then_requirement="M"`
///
/// An additional element check for compound trigger conditions (AND semantics).
///
/// Used for BDEW conditions like "STS+Z06+Z10" where the trigger requires
/// checking multiple element positions simultaneously.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AhbWhenElement {
    /// 0-based element index in the trigger segment.
    element_index: usize,
    /// Single value — the element must equal this value. Mutually exclusive with `value_alternatives`.
    #[serde(default)]
    value: Option<String>,
    /// OR alternatives — the element must equal ANY ONE of these values.
    /// Used for conditions like "STS+7++ZG9/ZH1/ZH2".
    #[serde(default)]
    value_alternatives: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct AhbConditionalRule {
    /// BDEW Bedingungsoperator category.
    ///
    /// Omitting this field defaults to `"I"` (implication) — the most common
    /// case where a trigger segment's presence makes the target mandatory.
    #[serde(default)]
    operator: AhbOperator,
    /// The segment tag to look for as the condition trigger (operand A).
    when_tag: String,
    /// Secondary segment tag for two-operand operators (X, U, O).
    ///
    /// Required when `operator` is `"X"`, `"U"`, or `"O"`.
    /// Optional for `"K"` (at most one); use `additional_tags` for K rules with ≥3 operands.
    #[serde(default)]
    secondary_tag: Option<String>,
    /// Extra segment tags beyond `when_tag` and `secondary_tag` for the `K` operator.
    ///
    /// Together with `when_tag` (and optionally `secondary_tag`) these form the
    /// complete set of mutually-exclusive tags.  At most one of all listed tags may appear.
    #[serde(default)]
    additional_tags: Vec<String>,
    /// For operator `"G"`: trigger when `when_tag` appears more than this many times.
    ///
    /// Defaults to 0 (trigger when present at least once, i.e. behaves like `"I"`).
    #[serde(default)]
    count_threshold: usize,
    /// 0-based element index in the trigger segment. Defaults to 0.
    #[serde(default)]
    when_element_index: usize,
    /// If present, condition fires only when the trigger has this value at `when_element_index`.
    /// If absent, any occurrence of `when_tag` triggers the condition.
    #[serde(default)]
    when_value: Option<String>,
    /// OR alternatives for `when_value` — the element at `when_element_index` must match ANY ONE.
    /// Mutually exclusive with `when_value`. Used for conditions like "QTY+67/201".
    #[serde(default)]
    when_value_alternatives: Vec<String>,
    /// Additional element checks (AND semantics). All must match for the condition to fire.
    /// Used for BDEW conditions like "STS+Z06+Z10+ZC1" that check multiple positions.
    #[serde(default)]
    when_additional_elements: Vec<AhbWhenElement>,
    /// M = this segment becomes mandatory when condition holds;
    /// N = this segment must not appear when condition holds.
    ///
    /// Not used for symmetric operators (X, U, O) — these always produce errors
    /// when their constraint is violated.
    #[serde(default)]
    then_requirement: String,
    /// If set, the mandatory check (then_requirement="M") requires that at least
    /// one occurrence of the target segment has this value at `then_qualifier_index`,
    /// rather than merely checking that the tag appears at all.
    ///
    /// Example: for STS with qualifier 9015=Z32, set
    ///   `then_qualifier_index=0, then_qualifier_value="Z32"`
    /// to generate: `!segments.iter().any(|s| s.tag == "STS" && s.element_str(0) == Some("Z32"))`
    #[serde(default)]
    then_qualifier_index: usize,
    #[serde(default)]
    then_qualifier_value: Option<String>,
    /// Human-readable BDEW condition text, e.g. "\[92\] Wenn QTY DE6063 mit Wert 67 vorhanden".
    #[serde(default, rename = "_description")]
    description: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AhbFieldRule {
    element: String,
    /// AHB requirement code for this field (M/O/N/C). Accepted from JSON schema;
    /// field-level requirement enforcement is not yet emitted (only `allowed_values` is used).
    #[expect(
        dead_code,
        reason = "field-level requirement enforcement not yet emitted; reserved for future value-presence checks"
    )]
    requirement: String,
    #[serde(default)]
    allowed_values: Vec<String>,
    /// 0-based element index within the segment.
    /// Must be set explicitly; there is no default to avoid silently checking
    /// the wrong element when the DE is not at position 0 (F-021).
    element_index: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodelistsData {
    #[serde(default)]
    schema_version: u32,
    release: String,
    lists: BTreeMap<String, Vec<String>>,
}

// ── Profile data ──────────────────────────────────────────────────────────────

struct ProfileData {
    message_type: String,
    /// The profile directory name — used as the Rust module name.
    /// For Formatversion-dated directories this is e.g. `"fv20240401"`;
    /// for legacy profiles it matches the assoc_code.
    folder_name: String,
    /// The wire-format assoc_code from UNH DE0057 (e.g. `"2.4c"`).
    /// Used as the registry lookup key — must match what real messages carry.
    release: String,
    /// Calendar date from which this profile is normatively valid, derived from
    /// the folder name when it matches `fv<YYYY><MM><DD>` (e.g. `fv20240401`).
    /// `None` for legacy folder names that do not embed a date.
    valid_from: Option<(u32, u8, u8)>, // (year, month, day)
    /// Last calendar date on which this profile is normatively valid.
    /// `None` means open-ended (no published expiry).
    valid_until: Option<String>,
    /// AHB revision identifier (may include letter-suffix corrections, e.g. "3.2e").
    ahb_revision: Option<String>,
    /// BDEW source document title, e.g. "MSCONS AHB 3.2, Stand 01.10.2025".
    source_document: Option<String>,
    /// Directory that this profile supersedes (same wire code, newer valid_from).
    supersedes_directory: Option<String>,
    /// When `true`, this profile is compiled only under the `{type}-archive` or
    /// `archive` Cargo features.  Set by `--prune-expired`; stored in `mig.json`.
    archived: bool,
    mig: MigProfile,
    ahb: AhbProfile,
    codelists: CodelistsData,
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Run code generation from `workspace_root`.
///
/// Supported flags in `args`:
/// - `--dry-run`: print what would be generated without writing files
/// - `--check`: diff generated content against committed files; exit 1 if any differ
/// - `--message-type <TYPE>`: only regenerate profiles for the given message type
/// - `--prune-expired [--grace-days N]`: mark profiles whose `valid_until + N` is in
///   the past as `archived = true` in their `mig.json`, then regenerate.  Default
///   grace period is 90 days.  The archived flag persists in JSON so `--check` remains
///   deterministic (does not depend on the current date).
pub fn run(workspace_root: &str, args: &[String]) {
    // Parse flags
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let check_mode = args.iter().any(|a| a == "--check");
    let prune_expired = args.iter().any(|a| a == "--prune-expired");
    let grace_days: u64 = args
        .windows(2)
        .find(|w| w[0] == "--grace-days")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(90);
    let message_type_filter: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--message-type")
        .map(|w| w[1].to_uppercase());

    let profiles_dir = PathBuf::from(workspace_root)
        .join("crates")
        .join("edi-energy")
        .join("profiles");
    let generated_dir = PathBuf::from(workspace_root)
        .join("crates")
        .join("edi-energy")
        .join("src")
        .join("generated");

    // ── Pre-codegen JSON Schema validation (F-015) ────────────────────────────
    // Validate all mig.json / ahb.json / codelists.json files against the JSON
    // Schema in profiles/schemas/ before generating any Rust code.  This catches
    // type mismatches and unknown fields that serde's deny_unknown_fields would
    // only surface as cryptic deserialization errors.
    //
    // Skip when a --message-type filter is active so partial codegen reruns are
    // still fast; CI always runs full codegen or validate-profiles separately.
    if message_type_filter.is_none() && !dry_run {
        let ok = crate::validate_profiles::run(workspace_root);
        if !ok {
            eprintln!(
                "xtask codegen: profile validation failed — fix the errors above before running codegen."
            );
            std::process::exit(1);
        }
        eprintln!("xtask codegen: profile schemas validated OK.");
    }

    let mut profiles = discover_profiles(&profiles_dir);

    // ── --prune-expired ───────────────────────────────────────────────────────
    // Mark profiles whose `valid_until + grace_days` is in the past as
    // `archived = true` in their `mig.json` files, then continue with full
    // codegen so `mod.rs` reflects the new archive status.
    //
    // Using an explicit JSON flag (rather than recomputing from date at every
    // codegen run) keeps `--check` deterministic: the generated mod.rs is
    // identical regardless of when `cargo xtask codegen` is run.
    if prune_expired {
        let pruned = apply_prune_expired(&profiles_dir, &profiles, grace_days);
        if pruned == 0 {
            eprintln!(
                "xtask codegen --prune-expired: no profiles to archive (grace_days={grace_days})."
            );
        } else {
            eprintln!(
                "xtask codegen --prune-expired: marked {pruned} profile(s) as archived in mig.json."
            );
            // Re-discover so archived flags are reflected in the in-memory profiles.
            profiles = discover_profiles(&profiles_dir);
        }
    }

    if let Some(ref filter) = message_type_filter {
        profiles.retain(|p| p.message_type.to_uppercase() == *filter);
        if profiles.is_empty() {
            eprintln!(
                "xtask codegen: no profiles found for message type {:?}",
                filter
            );
            std::process::exit(1);
        }
    }

    if profiles.is_empty() {
        eprintln!(
            "xtask codegen: no profiles found under {}",
            profiles_dir.display()
        );
        std::process::exit(1);
    }

    // Guard: module names must be unique — two profiles must not map to the same filename.
    {
        let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for p in &profiles {
            let mname = module_name(&p.message_type, &p.folder_name);
            let key = format!("{}::{}", p.message_type.to_lowercase(), p.release);
            if let Some(prev) = seen.insert(mname.clone(), key.clone()) {
                eprintln!(
                    "xtask codegen: module name collision `{mname}` between `{prev}` and `{key}`"
                );
                std::process::exit(1);
            }
        }
    }

    // Guard: same-wire-code pairs must have an explicit `supersedes_directory` link.
    // This enforces the AHB correction policy documented in F-001.
    {
        // Group profiles by (message_type, release) to find same-wire-code pairs.
        let mut wire_code_map: std::collections::HashMap<(String, String), Vec<&ProfileData>> =
            std::collections::HashMap::new();
        for p in &profiles {
            wire_code_map
                .entry((p.message_type.clone(), p.release.clone()))
                .or_default()
                .push(p);
        }
        for ((msg_type, wire_code), group) in &wire_code_map {
            if group.len() <= 1 {
                continue;
            }
            // Sort by valid_from ascending so we can check supersession chain.
            let mut sorted = group.clone();
            sorted.sort_by_key(|a| a.valid_from);
            for i in 1..sorted.len() {
                let newer = sorted[i];
                let older = sorted[i - 1];
                // The newer profile must declare that it supersedes the older directory.
                if newer.supersedes_directory.as_deref() != Some(&older.folder_name) {
                    eprintln!(
                        "  error: {msg_type} {wire_code}: profile `{}` (valid_from {:?}) \
                         supersedes `{}` (valid_from {:?}) but does not declare \
                         `supersedes_directory: {:?}` in mig.json — add this field to \
                         document the AHB correction chain (see F-001)",
                        newer.folder_name,
                        newer.valid_from,
                        older.folder_name,
                        older.valid_from,
                        older.folder_name,
                    );
                    std::process::exit(1);
                }
                // Also verify valid_from is strictly later.
                if newer.valid_from <= older.valid_from {
                    eprintln!(
                        "  error: {msg_type} {wire_code}: profile `{}` (valid_from {:?}) \
                         must have a strictly later valid_from than `{}` (valid_from {:?})",
                        newer.folder_name, newer.valid_from, older.folder_name, older.valid_from,
                    );
                    std::process::exit(1);
                }
            }
        }
    }

    if dry_run {
        eprintln!(
            "xtask codegen: dry-run — {} profile(s) would be generated:",
            profiles.len()
        );
        for p in &profiles {
            let module_name = module_name(&p.message_type, &p.folder_name);
            eprintln!(
                "  would write {module_name}.rs ({} → {})",
                p.message_type.to_lowercase(),
                p.release
            );
        }
        if message_type_filter.is_none() {
            eprintln!("  would write mod.rs");
        }
        return;
    }

    // ── --check mode ──────────────────────────────────────────────────────────
    // Regenerate all files in memory, apply rustfmt, and compare against the
    // committed versions. Exit 1 if any file is stale (CI drift guard).
    if check_mode {
        // --check mode requires rustfmt for a byte-for-byte comparison with the
        // on-disk formatted files.  Fail loudly rather than silently skipping the
        // comparison (F-022).
        let rustfmt_bin = which_rustfmt().unwrap_or_else(|| {
            eprintln!(
                "error: rustfmt is required for `--check` mode but was not found.\n\
                 Install it with: rustup component add rustfmt"
            );
            std::process::exit(1);
        });
        let mut stale: Vec<String> = Vec::new();
        for p in &profiles {
            let module_name = module_name(&p.message_type, &p.folder_name);
            let file_name = format!("{module_name}.rs");
            let path = generated_dir.join(&file_name);
            let raw = emit_profile_module(p);
            // Format through rustfmt for an apples-to-apples comparison with the on-disk file.
            let expected = rustfmt_string(&rustfmt_bin, raw).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            match std::fs::read_to_string(&path) {
                Ok(actual) if actual == expected => {}
                Ok(_) => {
                    eprintln!("  stale: {file_name}");
                    stale.push(file_name);
                }
                Err(_) => {
                    eprintln!("  missing: {file_name}");
                    stale.push(file_name);
                }
            }
        }
        if message_type_filter.is_none() {
            let all_profiles = discover_profiles(&profiles_dir);
            let raw_mod = emit_mod_rs(&all_profiles);
            let expected_mod = rustfmt_string(&rustfmt_bin, raw_mod).unwrap_or_else(|e| {
                eprintln!("error: {e}");
                std::process::exit(1);
            });
            match std::fs::read_to_string(generated_dir.join("mod.rs")) {
                Ok(actual) if actual == expected_mod => {}
                Ok(_) => {
                    eprintln!("  stale: mod.rs");
                    stale.push("mod.rs".to_owned());
                }
                Err(_) => {
                    eprintln!("  missing: mod.rs");
                    stale.push("mod.rs".to_owned());
                }
            }
        }
        if stale.is_empty() {
            eprintln!("xtask codegen --check: all generated files are up to date.");
        } else {
            eprintln!(
                "xtask codegen --check: {} file(s) are stale — run `cargo xtask codegen` to regenerate.",
                stale.len()
            );
            std::process::exit(1);
        }

        // Also verify message_type.rs has from_unh_code() arms for every registered message type.
        // This catches the F-006 drift where a new profile is added but message_type.rs is not updated.
        let message_type_rs = PathBuf::from(workspace_root)
            .join("crates")
            .join("edi-energy")
            .join("src")
            .join("message_type.rs");
        if let Ok(mt_src) = std::fs::read_to_string(&message_type_rs) {
            let all_profiles = discover_profiles(&profiles_dir);
            let known_types: std::collections::BTreeSet<String> = all_profiles
                .iter()
                .map(|p| p.message_type.to_uppercase())
                .collect();
            let mut dispatch_errors: Vec<String> = Vec::new();
            for mt in &known_types {
                // Check that from_unh_code() has an arm for this type
                let arm = format!("\"{}\"", mt);
                if !mt_src.contains(&arm) {
                    dispatch_errors.push(format!(
                        "  error: MessageType::from_unh_code() missing arm for {mt:?} — \
                         add it to crates/edi-energy/src/message_type.rs (F-006)"
                    ));
                }
                // Check that as_str() has an arm for this type
                if !mt_src.contains(&format!("=> \"{mt}\"")) {
                    dispatch_errors.push(format!(
                        "  error: MessageType::as_str() missing arm for {mt:?} — \
                         add it to crates/edi-energy/src/message_type.rs (F-006)"
                    ));
                }
            }
            if dispatch_errors.is_empty() {
                eprintln!(
                    "xtask codegen --check: MessageType dispatch covers all {} registered types.",
                    known_types.len()
                );
            } else {
                for e in &dispatch_errors {
                    eprintln!("{e}");
                }
                std::process::exit(1);
            }
        }

        return;
    }

    eprintln!(
        "xtask codegen: generating {} profile(s) into {}",
        profiles.len(),
        generated_dir.display()
    );

    let mut written = 0u32;
    for p in &profiles {
        let module_name = module_name(&p.message_type, &p.folder_name);
        let file_name = format!("{module_name}.rs");
        let path = generated_dir.join(&file_name);
        let src = emit_profile_module(p);
        if write_file_if_changed(&path, &src) {
            written += 1;
        }
        eprintln!(
            "  wrote {} ({} -> {})",
            file_name,
            p.message_type.to_lowercase(),
            p.release
        );
    }

    // Emit mod.rs only when regenerating everything (not filtered to a single type)
    if message_type_filter.is_none() {
        // We need all profiles (not just the filtered subset) to build mod.rs correctly.
        let all_profiles = discover_profiles(&profiles_dir);
        let mod_src = emit_mod_rs(&all_profiles);
        if write_file_if_changed(&generated_dir.join("mod.rs"), &mod_src) {
            written += 1;
        }
        eprintln!("  wrote mod.rs");
    }

    // Format the generated files
    format_generated(&generated_dir);

    // Verify message_type.rs dispatch coverage (F-006 guard).
    // This is a warning-only check in normal mode; --check mode exits 1 on failure.
    if message_type_filter.is_none() {
        let message_type_rs = PathBuf::from(workspace_root)
            .join("crates")
            .join("edi-energy")
            .join("src")
            .join("message_type.rs");
        if let Ok(mt_src) = std::fs::read_to_string(&message_type_rs) {
            let all = discover_profiles(&profiles_dir);
            let known: std::collections::BTreeSet<String> =
                all.iter().map(|p| p.message_type.to_uppercase()).collect();
            for mt in &known {
                if !mt_src.contains(&format!("\"{}\"", mt)) {
                    eprintln!(
                        "  warning: MessageType::from_unh_code() may be missing arm for {mt:?} \
                         — update crates/edi-energy/src/message_type.rs (F-006)"
                    );
                }
            }
        }
    }

    eprintln!(
        "xtask codegen: done. Generated {} file(s) for {} (type, release) pair(s).",
        written,
        profiles.len()
    );
}

// ── Discovery ─────────────────────────────────────────────────────────────────

fn discover_profiles(profiles_dir: &Path) -> Vec<ProfileData> {
    let mut result = Vec::new();

    let message_types = read_subdirs(profiles_dir);
    for msg_type_dir in message_types {
        let message_type = msg_type_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_uppercase();

        if message_type == "SCHEMAS" {
            continue;
        }

        let release_dirs = read_subdirs(&msg_type_dir);
        for release_dir in release_dirs {
            // The directory name is the "folder_name" used for module naming.
            // For Formatversion-dated profiles this is e.g. "fv20251001";
            // for legacy profiles it may equal the assoc_code.
            let folder_name = release_dir
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string();

            let mig_path = release_dir.join("mig.json");
            let ahb_path = release_dir.join("ahb.json");
            let codelists_path = release_dir.join("codelists.json");

            if !mig_path.exists() || !ahb_path.exists() || !codelists_path.exists() {
                eprintln!(
                    "  warning: skipping {} {} — missing one or more JSON files",
                    message_type, folder_name
                );
                continue;
            }

            let mig: MigProfile = load_json(&mig_path);
            let ahb: AhbProfile = load_json(&ahb_path);
            let codelists: CodelistsData = load_json(&codelists_path);

            // Cross-check that all three JSON files agree on the release (wire assoc_code).
            // The release does NOT need to match the directory name (folder_name), since
            // FV-dated directories like "fv20251001" carry a different assoc_code "2.4c".
            if mig.release != ahb.release {
                eprintln!(
                    "  error: {} {folder_name}: mig.json release '{}' differs from ahb.json release '{}'",
                    message_type, mig.release, ahb.release
                );
                std::process::exit(1);
            }
            if mig.release != codelists.release {
                eprintln!(
                    "  error: {} {folder_name}: mig.json release '{}' differs from codelists.json release '{}'",
                    message_type, mig.release, codelists.release
                );
                std::process::exit(1);
            }
            let release = mig.release.clone();

            // Schema version range check.
            //
            // All profile files must have a `schema_version` in [MIN..=MAX].
            //
            // MIN_SCHEMA_VERSION: the oldest schema version this codegen can read.
            //   Profiles with a version below MIN require migration (update the JSON).
            //
            // MAX_SCHEMA_VERSION: the newest schema version this codegen understands.
            //   Profiles with a version above MAX are authored for a newer codegen;
            //   update xtask/src/codegen.rs to support the new version.
            //
            // Additive-only changes (new optional fields with `#[serde(default)]`)
            // do NOT require a version bump — they are forward-compatible under the
            // `deny_unknown_fields` policy because the field is absent in old JSON and
            // defaults gracefully.  Only structural (breaking) changes warrant a bump.
            const MIN_SCHEMA_VERSION: u32 = 1;
            const MAX_SCHEMA_VERSION: u32 = 1;
            for (name, version) in [
                ("mig.json", mig.schema_version),
                ("ahb.json", ahb.schema_version),
                ("codelists.json", codelists.schema_version),
            ] {
                if version < MIN_SCHEMA_VERSION {
                    eprintln!(
                        "  error: {} {folder_name} {name} has schema_version {version} \
                         (minimum is {MIN_SCHEMA_VERSION}) — update the profile JSON file \
                         to at least schema_version {MIN_SCHEMA_VERSION}",
                        message_type
                    );
                    std::process::exit(1);
                }
                if version > MAX_SCHEMA_VERSION {
                    eprintln!(
                        "  error: {} {folder_name} {name} has schema_version {version} \
                         (maximum supported is {MAX_SCHEMA_VERSION}) — this profile was \
                         authored for a newer codegen; update xtask/src/codegen.rs to \
                         support schema version {version}",
                        message_type
                    );
                    std::process::exit(1);
                }
            }

            // Prefer the explicit `valid_from` field in mig.json over the directory-name
            // derivation.  The directory-name fallback exists only for legacy profiles
            // created before F-020 added the explicit field.
            let valid_from = mig
                .valid_from
                .as_deref()
                .and_then(parse_iso_date)
                .or_else(|| parse_fv_date(&folder_name));

            // Cross-check: if both sources are present they must agree.
            if let (Some(from_json), Some(from_dir)) = (
                mig.valid_from.as_deref().and_then(parse_iso_date),
                parse_fv_date(&folder_name),
            ) && from_json != from_dir
            {
                eprintln!(
                    "  error: {} {folder_name}: mig.json valid_from '{}' does not \
                         match the date implied by the directory name ({}-{:02}-{:02})",
                    message_type,
                    mig.valid_from.as_deref().unwrap_or(""),
                    from_dir.0,
                    from_dir.1,
                    from_dir.2,
                );
                std::process::exit(1);
            }

            let valid_until = mig.valid_until.clone();
            let ahb_revision = mig.ahb_revision.clone();
            let source_document = mig.source_document.clone();
            let supersedes_directory = mig.supersedes_directory.clone();
            let archived = mig.archived;

            result.push(ProfileData {
                message_type: message_type.clone(),
                folder_name,
                release,
                valid_from,
                valid_until,
                ahb_revision,
                source_document,
                supersedes_directory,
                archived,
                mig,
                ahb,
                codelists,
            });
        }
    }

    // Sort for deterministic output
    result
        .sort_by(|a, b| (&a.message_type, &a.folder_name).cmp(&(&b.message_type, &b.folder_name)));
    result
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

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let content = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error: cannot read {}: {}", path.display(), e);
        std::process::exit(1);
    });
    serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!("error: cannot parse {}: {}", path.display(), e);
        std::process::exit(1);
    })
}

// ── Naming helpers ────────────────────────────────────────────────────────────

/// Convert `("UTILMD", "5.5.3a")` → `"utilmd_5_5_3a"`.
/// Derive the Rust module name from the message type and the profile **folder name**.
///
/// The folder name is the directory component under `profiles/<type>/` — for
/// Formatversion-dated profiles this is e.g. `"fv20251001"`, for legacy
/// profiles it typically equals the assoc_code (e.g. `"2.4c"`).
fn module_name(message_type: &str, folder_name: &str) -> String {
    let type_part = message_type.to_lowercase();
    let folder_part = folder_name.replace(['.', '-'], "_");
    format!("{type_part}_{folder_part}")
}

/// Convert `("UTILMD", "fv20251001")` → `"UtilmdFv20251001Profile"` (struct name).
///
/// The struct name is derived from the **folder name** (not the wire release code)
/// to guarantee uniqueness: two directories with the same wire release code
/// (e.g. an AHB correction cycle) map to different struct names.
fn struct_name(message_type: &str, folder_name: &str) -> String {
    let type_part = {
        let mut s = message_type.to_lowercase();
        if let Some(c) = s.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        s
    };
    // PascalCase-ify the folder name: split on non-alphanumeric, title-case each word.
    let folder_part: String = folder_name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut s = first.to_uppercase().to_string();
                    s.push_str(chars.as_str());
                    s
                }
            }
        })
        .collect();
    format!("{type_part}{folder_part}Profile")
}

/// Cargo feature name for a message type, e.g. `"utilmd"`.
fn feature_name(message_type: &str) -> String {
    message_type.to_lowercase()
}

/// Cargo feature name for the archive gate of a message type.
///
/// The archive feature is off by default so expired profiles do not inflate
/// compile times for users who only need current releases.
///
/// Convention: `{lowercase_type}-archive`, e.g. `"mscons-archive"`.
fn archive_feature_name(message_type: &str) -> String {
    format!("{}-archive", message_type.to_lowercase())
}

/// Parse an ISO 8601 date string `"YYYY-MM-DD"` into `(year, month, day)`.
/// Returns `None` for any malformed input.
fn parse_date_str(s: &str) -> Option<(u32, u8, u8)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: u32 = parts[0].parse().ok()?;
    let month: u8 = parts[1].parse().ok()?;
    let day: u8 = parts[2].parse().ok()?;
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    Some((year, month, day))
}

/// Return today's date as `(year, month, day)` using the system clock.
fn today_ymd() -> (u32, u8, u8) {
    // Use std::time to avoid a new dependency.
    // SystemTime gives seconds since UNIX epoch; convert to a date.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Days since Unix epoch (1970-01-01)
    let days_since_epoch = secs / 86400;

    // Convert days_since_epoch to (year, month, day) using a compact algorithm.
    // Based on the proleptic Gregorian calendar algorithm by Richards (2013).
    let z = days_since_epoch + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u32, m as u8, d as u8)
}

/// Compare two `(year, month, day)` tuples.
///
/// Returns `true` when `a` is strictly before `b` in calendar time.
fn date_before(a: (u32, u8, u8), b: (u32, u8, u8)) -> bool {
    a < b
}

/// Convert a `(year, month, day)` date to days since the Unix epoch (1970-01-01).
///
/// Uses Howard Hinnant's civil-time algorithm (inverse of `today_ymd`).
fn date_to_unix_days(year: u32, month: u8, day: u8) -> u64 {
    let d = day as i64;
    let m = month as i64;
    let mut y = year as i64;
    // Shift Jan/Feb to months 13/14 of the previous year so the algorithm treats
    // March 1 as the start of each "year cycle".
    y -= if m <= 2 { 1 } else { 0 };
    let era = y / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = ((153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1) as u64; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    (era * 146097 + doe as i64 - 719468) as u64
}

/// Add `days` to a `(year, month, day)` date.
fn add_days(date: (u32, u8, u8), days: u64) -> (u32, u8, u8) {
    let unix_days = date_to_unix_days(date.0, date.1, date.2);
    let total = unix_days + days;

    // Convert back to (y, m, d) using Howard Hinnant's algorithm (same as today_ymd).
    let z = total + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y as u32, m as u8, d as u8)
}

/// Mark profiles as `archived = true` in their `mig.json` files when their
/// `valid_until + grace_days` date is strictly before today.
///
/// Returns the count of profiles that were updated.
///
/// # Design
///
/// The `archived` flag is persisted in the profile JSON file \u2014 not recomputed
/// from `valid_until` at every codegen run.  This keeps `cargo xtask codegen
/// --check` deterministic: the generated `mod.rs` depends only on the JSON
/// files, not on the current date.  The annual workflow is:
///
/// 1. Run `cargo xtask codegen --prune-expired` to set `"archived": true` in
///    expired profiles.
/// 2. Commit the updated `mig.json` files AND the regenerated `mod.rs`.
/// 3. `--check` in CI compares against the committed output and stays green
///    until the next annual run.
fn apply_prune_expired(profiles_dir: &Path, profiles: &[ProfileData], grace_days: u64) -> u32 {
    let today = today_ymd();
    let mut count = 0u32;

    for p in profiles {
        // Skip profiles that are already archived or have no valid_until.
        if p.archived {
            continue;
        }
        let valid_until = match &p.valid_until {
            Some(s) => s,
            None => continue,
        };
        let valid_until_date = match parse_date_str(valid_until) {
            Some(d) => d,
            None => {
                eprintln!(
                    "  warning: {} {} has malformed valid_until {:?} — skipping prune check",
                    p.message_type, p.folder_name, valid_until
                );
                continue;
            }
        };
        // Archive when valid_until + grace_days < today.
        let cutoff = add_days(valid_until_date, grace_days);
        if date_before(cutoff, today) {
            // Set "archived": true in the mig.json file using a simple string replacement
            // (avoids a full JSON round-trip that would reformat the file).
            let mig_path = profiles_dir
                .join(p.message_type.to_lowercase())
                .join(&p.folder_name)
                .join("mig.json");
            let content = std::fs::read_to_string(&mig_path).unwrap_or_else(|e| {
                eprintln!("error: cannot read {}: {}", mig_path.display(), e);
                std::process::exit(1);
            });

            // If "archived" is already present (false), replace it; otherwise
            // inject it before the first field so the file stays readable.
            let updated = if content.contains("\"archived\"") {
                content.replace("\"archived\": false", "\"archived\": true")
            } else {
                // Inject after the opening `{` + optional whitespace / newline.
                // Find the position right after the opening brace.
                let insert_at = content.find('{').map(|i| i + 1).unwrap_or(0);
                let (before, after) = content.split_at(insert_at);
                // Determine indentation from the first field line.
                let indent = after
                    .lines()
                    .find(|l| l.trim_start().starts_with('"'))
                    .map(|l| {
                        let ws: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                        ws
                    })
                    .unwrap_or_else(|| "  ".to_owned());
                format!("{before}\n{indent}\"archived\": true,{after}")
            };

            std::fs::write(&mig_path, &updated).unwrap_or_else(|e| {
                eprintln!("error: cannot write {}: {}", mig_path.display(), e);
                std::process::exit(1);
            });
            eprintln!(
                "  archived: {} {} (valid_until={}, cutoff={:04}-{:02}-{:02})",
                p.message_type, p.folder_name, valid_until, cutoff.0, cutoff.1, cutoff.2,
            );
            count += 1;
        }
    }
    count
}

/// Parse a `valid_from` date from an ISO 8601 string (`"YYYY-MM-DD"`).
///
/// Returns `Some((year, month, day))` for a well-formed date, `None` otherwise.
fn parse_iso_date(s: &str) -> Option<(u32, u8, u8)> {
    let bytes = s.as_bytes();
    if bytes.len() < 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: u32 = s[0..4].parse().ok()?;
    let month: u8 = s[5..7].parse().ok()?;
    let day: u8 = s[8..10].parse().ok()?;
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    Some((year, month, day))
}

/// Parse a `valid_from` date from a folder name of the form `fv<YYYY><MM><DD>[_<suffix>]`.
///
/// Returns `Some((year, month, day))` for well-formed FV-date directories,
/// `None` for legacy names (e.g. `"2.4c"`, `"5.5.3a"`).
///
/// The optional `_<suffix>` (e.g. `"_gas"`) is stripped before parsing,
/// allowing multi-track profiles with the same effective date.
fn parse_fv_date(folder_name: &str) -> Option<(u32, u8, u8)> {
    let after_fv = folder_name.strip_prefix("fv")?;
    // Take exactly 8 digits; any trailing `_<suffix>` is ignored.
    let digits = if after_fv.len() >= 8 {
        &after_fv[..8]
    } else {
        return None;
    };
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Remainder after the 8 digits must be empty or start with `_`.
    let rest = &after_fv[8..];
    if !rest.is_empty() && !rest.starts_with('_') {
        return None;
    }
    let year: u32 = digits[0..4].parse().ok()?;
    let month: u8 = digits[4..6].parse().ok()?;
    let day: u8 = digits[6..8].parse().ok()?;
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    Some((year, month, day))
}

// ── Code emission ─────────────────────────────────────────────────────────────

fn emit_profile_module(p: &ProfileData) -> String {
    let _module = module_name(&p.message_type, &p.folder_name);
    let struct_name = struct_name(&p.message_type, &p.folder_name);
    let feature = feature_name(&p.message_type);

    let mut out = String::new();

    // File header — the module IS this file; the #[cfg] gate lives on the `mod`
    // declaration in generated/mod.rs, not in the file itself.
    writeln!(
        out,
        "// @generated — do not edit by hand; run `cargo xtask codegen` to regenerate"
    )
    .unwrap();
    // Suppress doc_markdown: generated condition descriptions contain BDEW terms
    // like "WiM", "GPKE", "MaBiS" that clippy wants in backticks.
    writeln!(out, "#![allow(clippy::doc_markdown)]").unwrap();
    writeln!(out).unwrap();
    // Codegen schema version constant (F-040): enables runtime/CI detection of
    // profiles that were generated by an incompatible codegen version.
    // The value matches the MAX_SCHEMA_VERSION accepted by this codegen.
    writeln!(
        out,
        "/// Codegen schema version this module was generated from."
    )
    .unwrap();
    writeln!(
        out,
        "/// Compared against `mig.json` `schema_version` in CI to detect drift."
    )
    .unwrap();
    writeln!(out, "pub(crate) const CODEGEN_SCHEMA_VERSION: u32 = 1;").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "use std::sync::{{Arc, LazyLock}};").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "use edifact_rs::directory_validator::{{ElementRef, SegmentDefinition, Status}};"
    )
    .unwrap();
    writeln!(out, "use edifact_rs::{{DirectoryValidator, GroupDef, ProfileRulePack, ValidationIssue, ValidationSeverity}};").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "use crate::registry::Profile;").unwrap();
    writeln!(
        out,
        "use crate::{{MessageType, Pruefidentifikator, Release}};"
    )
    .unwrap();
    writeln!(out).unwrap();

    // Collect all segments (flat + from groups)
    let all_segments = collect_all_segments(&p.mig);

    // Emit static SEGMENTS array
    emit_segments_array(&mut out, &all_segments);

    // Emit segment_lookup fn
    emit_segment_lookup(&mut out);

    // Emit code list statics
    emit_codelists_statics(&mut out, &p.codelists);

    // Emit is_code_valid
    emit_is_code_valid(&mut out, &p.codelists);

    // Emit suggest_code
    emit_suggest_code(&mut out, &p.codelists);

    // Emit expected_components
    emit_expected_components(&mut out, &all_segments);

    // Emit code_list fn
    emit_code_list_fn(&mut out, &p.codelists);

    // Emit DirectoryValidator factory
    emit_directory_validator_fn(&mut out, &p.message_type, &p.release);

    // Emit MIG rule pack
    emit_mig_rule_pack(&mut out, &p.mig);

    // Emit segment group schema static (used by validate_lenient_grouped_owned)
    emit_group_schema_static(&mut out, &p.mig.segment_groups, &p.ahb);

    // Emit AHB rule functions + pack
    emit_ahb_rule_pack(&mut out, &p.ahb, &p.mig.message_type);

    // Emit concrete Profile struct + impl
    emit_profile_impl(&mut out, p, &struct_name, &feature);

    out
}

// ── Segment collection ────────────────────────────────────────────────────────

fn collect_all_segments(mig: &MigProfile) -> Vec<&MigSegment> {
    let mut result = Vec::new();
    for seg in &mig.segments {
        result.push(seg);
    }
    for group in &mig.segment_groups {
        collect_group_segments(group, &mut result);
    }
    result
}

/// Collect segments that are *globally* mandatory — i.e. they must appear in
/// every well-formed message regardless of which optional groups are present.
///
/// A segment is globally mandatory only when the entire group chain leading to
/// it is mandatory (`group.mandatory == true`) AND the segment itself is
/// mandatory (`seg.mandatory == true`).  Segments inside optional groups are
/// conditionally mandatory (they must appear *when* the group appears) but
/// must not generate a global presence check.
fn collect_globally_mandatory_segments(mig: &MigProfile) -> Vec<&MigSegment> {
    let mut result = Vec::new();
    for seg in &mig.segments {
        if seg.mandatory {
            result.push(seg);
        }
    }
    for group in &mig.segment_groups {
        collect_mandatory_group_segments(group, group.mandatory, &mut result);
    }
    result
}

fn collect_mandatory_group_segments<'a>(
    group: &'a MigGroup,
    group_chain_mandatory: bool,
    result: &mut Vec<&'a MigSegment>,
) {
    for seg in &group.segments {
        if group_chain_mandatory && seg.mandatory {
            result.push(seg);
        }
    }
    for nested in &group.groups {
        collect_mandatory_group_segments(nested, group_chain_mandatory && nested.mandatory, result);
    }
}

fn collect_group_segments<'a>(group: &'a MigGroup, result: &mut Vec<&'a MigSegment>) {
    for seg in &group.segments {
        result.push(seg);
    }
    for nested in &group.groups {
        collect_group_segments(nested, result);
    }
}

/// Collect the set of all segment tags that appear inside ANY segment group
/// (at any nesting depth).  Used by F-010 fix to skip global cardinality rules
/// for tags that also appear in groups.
fn collect_all_group_tags(groups: &[MigGroup]) -> std::collections::HashSet<String> {
    let mut tags = std::collections::HashSet::new();
    for group in groups {
        for seg in &group.segments {
            tags.insert(seg.tag.clone());
        }
        let nested = collect_all_group_tags(&group.groups);
        tags.extend(nested);
    }
    tags
}

// ── Segment definitions ───────────────────────────────────────────────────────

fn emit_segments_array(out: &mut String, segments: &[&MigSegment]) {
    writeln!(out, "    static SEGMENTS: &[SegmentDefinition] = &[").unwrap();
    let mut seen_tags = std::collections::HashSet::new();
    for seg in segments {
        if !seen_tags.insert(seg.tag.as_str()) {
            continue; // skip duplicate tags (same segment in multiple groups)
        }
        writeln!(out, "        SegmentDefinition {{").unwrap();
        writeln!(out, "            tag: {:?},", seg.tag).unwrap();
        writeln!(out, "            name: {:?},", seg.name).unwrap();
        writeln!(out, "            elements: &[").unwrap();
        for (pos, elem) in seg.elements.iter().enumerate() {
            let status = if elem.status == "M" {
                "Status::Mandatory"
            } else {
                "Status::Conditional"
            };
            writeln!(
                out,
                "                ElementRef::new({}, {:?}, {}, 1),",
                pos + 1,
                elem.id,
                status
            )
            .unwrap();
        }
        writeln!(out, "            ],").unwrap();
        writeln!(out, "        }},").unwrap();
    }
    writeln!(out, "    ];").unwrap();
    writeln!(out).unwrap();
}

fn emit_segment_lookup(out: &mut String) {
    writeln!(
        out,
        "    static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> ="
    )
    .unwrap();
    writeln!(
        out,
        "        LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {{"
    )
    .unwrap();
    writeln!(out, "        SEGMENT_MAP.get(tag).copied()").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

// ── Code lists ────────────────────────────────────────────────────────────────

fn emit_codelists_statics(out: &mut String, codelists: &CodelistsData) {
    for (de_id, values) in &codelists.lists {
        let const_name = format!("CODES_{}", de_id.replace('-', "_").to_uppercase());
        write!(out, "    static {const_name}: &[&str] = &[").unwrap();
        let mut sorted = values.clone();
        sorted.sort();
        for (i, v) in sorted.iter().enumerate() {
            if i > 0 {
                write!(out, ", ").unwrap();
            }
            write!(out, "{:?}", v).unwrap();
        }
        writeln!(out, "];").unwrap();
    }
    writeln!(out).unwrap();
}

fn emit_is_code_valid(out: &mut String, _codelists: &CodelistsData) {
    // F-026 fix: delegate to code_list() instead of duplicating match arms.
    // code_list() already has the single authoritative per-DE dispatch;
    // `is_code_valid` just binary-searches the returned slice.
    // Unknown DE ids return None → treated as valid (open set assumption).
    writeln!(
        out,
        "    pub(crate) fn is_code_valid(de_id: &str, code: &str) -> bool {{"
    )
    .unwrap();
    writeln!(
        out,
        "        code_list(de_id).is_none_or(|codes| codes.binary_search(&code).is_ok())"
    )
    .unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn emit_suggest_code(out: &mut String, _codelists: &CodelistsData) {
    // F-013 fix: use the `code` argument to find the lexicographically nearest valid code.
    // `partition_point` finds the first index where the sorted code list value >= `code`.
    // This is the closest valid code by lexicographic order — much more useful than
    // always returning the first code regardless of the invalid input.
    // Using `code_list()` avoids duplicating the per-DE match arms here.
    writeln!(
        out,
        "    pub(crate) fn suggest_code(de_id: &str, code: &str) -> Option<&'static str> {{"
    )
    .unwrap();
    writeln!(out, "        let codes = code_list(de_id)?;").unwrap();
    writeln!(
        out,
        "        // Return the lexicographically nearest valid code."
    )
    .unwrap();
    writeln!(
        out,
        "        // partition_point gives the insertion point for `code` in the sorted slice,"
    )
    .unwrap();
    writeln!(
        out,
        "        // so codes[idx] is the first valid code >= code (or last if past end)."
    )
    .unwrap();
    writeln!(
        out,
        "        let idx = codes.partition_point(|&c| c < code);"
    )
    .unwrap();
    writeln!(
        out,
        "        codes.get(idx).or_else(|| codes.last()).copied()"
    )
    .unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn emit_expected_components(out: &mut String, segments: &[&MigSegment]) {
    writeln!(
        out,
        "    fn expected_components(tag: &str, idx: usize) -> Option<u8> {{"
    )
    .unwrap();
    writeln!(out, "        match (tag, idx) {{").unwrap();

    // Collect all (tag, idx) → components mappings.
    let mut arms: Vec<(String, usize, u8)> = Vec::new();
    let mut seen_tags = std::collections::HashSet::new();
    for seg in segments {
        if !seen_tags.insert(seg.tag.as_str()) {
            continue;
        }
        for (i, elem) in seg.elements.iter().enumerate() {
            if let Some(components) = elem.components {
                if components == 1 {
                    arms.push((seg.tag.clone(), i, components as u8));
                }
            }
        }
    }

    // Group arms by return value and emit merged `pattern | pattern => value` arms
    // to avoid `clippy::match_same_arms`.
    let mut by_value: std::collections::BTreeMap<u8, Vec<(String, usize)>> =
        std::collections::BTreeMap::new();
    for (tag, idx, val) in arms {
        by_value.entry(val).or_default().push((tag, idx));
    }
    for (val, patterns) in &by_value {
        let joined = patterns
            .iter()
            .map(|(tag, idx)| format!("({:?}, {})", tag, idx))
            .collect::<Vec<_>>()
            .join(" | ");
        writeln!(out, "            {joined} => Some({val}),").unwrap();
    }

    writeln!(out, "            _ => None,").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn emit_code_list_fn(out: &mut String, codelists: &CodelistsData) {
    writeln!(
        out,
        "    pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {{"
    )
    .unwrap();
    writeln!(out, "        match de_id {{").unwrap();
    for de_id in codelists.lists.keys() {
        let const_name = format!("CODES_{}", de_id.replace('-', "_").to_uppercase());
        writeln!(out, "            {:?} => Some({const_name}),", de_id).unwrap();
    }
    writeln!(out, "            _ => None,").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

// ── DirectoryValidator factory ────────────────────────────────────────────────

fn emit_directory_validator_fn(out: &mut String, message_type: &str, release: &str) {
    let dir_id = format!("EDI@Energy-{message_type}-{release}");
    let static_name = format!(
        "DIRECTORY_VALIDATOR_{}_{}",
        message_type.replace('-', "_").to_uppercase(),
        release.replace(['.', '-'], "_").to_uppercase()
    );
    writeln!(
        out,
        "    // Layer 2 scope: mandatory segment presence, element/component counts,"
    )
    .unwrap();
    writeln!(
        out,
        "    // code-list validity. Does NOT check segment sequence or repetition"
    )
    .unwrap();
    writeln!(
        out,
        "    // cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities."
    )
    .unwrap();
    writeln!(
        out,
        "    // Cached in a LazyLock so construction happens once per profile (F-019 fix)."
    )
    .unwrap();
    writeln!(
        out,
        "    static {static_name}: LazyLock<DirectoryValidator> = LazyLock::new(|| {{"
    )
    .unwrap();
    writeln!(
        out,
        "        DirectoryValidator::new({dir_id:?}, segment_lookup, is_code_valid, suggest_code, expected_components, None)"
    )
    .unwrap();
    writeln!(out, "    }});").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub(crate) fn directory_validator() -> &'static DirectoryValidator {{"
    )
    .unwrap();
    writeln!(out, "        &{static_name}").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

// ── MIG rule pack ─────────────────────────────────────────────────────────────

/// Derive the expected top-level sequence of segment tags from a MIG profile.
///
/// The sequence covers three categories of tags, in order:
///
/// 1. **Top-level header segments** (`mig.segments`, minus `UNT`): e.g. `UNH`,
///    `BGM`, `DTM` in UTILMD.
///
/// 2. **Top-level group trigger segments** (first level of `mig.segment_groups`
///    only): e.g. `RFF` (SG1), `NAD` (SG2), `IDE` (SG4) in UTILMD.
///    Nested group triggers (SG5, SG6 …) are group-internal and must **not**
///    appear in the flat ordering check — they only appear within each parent
///    group occurrence.
///    Trigger tags that are already in the top-level header (e.g. `DTM` when it
///    appears both as a header segment and as a group-internal segment) are
///    **excluded** to avoid false "out-of-order" violations: including a tag at
///    cursor position *k* and then seeing it again group-internally (after the
///    cursor advanced past *k*) would produce a spurious violation.
///
/// 3. **`UNT`**: always the final element.
///
/// When the profile contains a non-empty `ordering_hint`, that list is returned
/// verbatim (the hint is the authoritative override for unusual message layouts,
/// e.g. MSCONS where a `UNS` section-control segment appears between headers and
/// detail groups).
fn mig_segment_sequence(mig: &MigProfile) -> Vec<String> {
    // Use the explicit hint when provided.
    if !mig.ordering_hint.is_empty() {
        return mig.ordering_hint.clone();
    }

    // Build the set of top-level tags so we can exclude trigger tags that would
    // create ambiguity with group-internal occurrences.
    let toplevel_tags: std::collections::HashSet<&str> =
        mig.segments.iter().map(|s| s.tag.as_str()).collect();

    // Step 1 — top-level header segments in declared order, excluding UNT.
    // Dedup while preserving order (PARTIN has repeated tags in mig.segments).
    let mut ordered: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for seg in &mig.segments {
        if seg.tag == "UNT" {
            continue; // UNT is appended last
        }
        if seen.insert(seg.tag.clone()) {
            ordered.push(seg.tag.clone());
        }
    }

    // Step 2 — first-level group trigger segments (non-recursive).
    // A trigger tag that also appears in the top-level header is skipped to
    // prevent false "out-of-order" violations on group-internal occurrences.
    for group in &mig.segment_groups {
        if !toplevel_tags.contains(group.trigger_segment.as_str())
            && seen.insert(group.trigger_segment.clone())
        {
            ordered.push(group.trigger_segment.clone());
        }
    }

    // Step 3 — UNT is always last.
    if mig.segments.iter().any(|s| s.tag == "UNT") {
        ordered.push("UNT".to_owned());
    }

    ordered
}

// ── Segment-group schema ──────────────────────────────────────────────────────

/// Collect the set of SG IDs that are directly referenced as `group_id` in any
/// PID's `group_rules` across the entire AHB profile.
fn collect_relevant_group_ids(ahb: &AhbProfile) -> std::collections::HashSet<&str> {
    let mut ids = std::collections::HashSet::new();
    for pid in &ahb.pruefidentifikatoren {
        for gr in &pid.group_rules {
            ids.insert(gr.group_id.as_str());
        }
    }
    ids
}

/// Return `true` if `group` or any of its descendants is referenced in
/// `relevant_ids`.  Used to prune the GROUP_SCHEMA to a minimal tree that only
/// includes paths leading to groups with actual rules.
///
/// Pruning is essential when two sibling groups share the same trigger tag:
/// `group_segments_indexed` picks the first schema match, so including an
/// irrelevant sibling before a relevant one prevents the relevant one from
/// ever being reached.  (MSCONS: SG2 and SG5 both trigger on NAD; SG5 leads
/// to SG10 which has group rules, SG2 does not.)
fn group_is_relevant(group: &MigGroup, relevant_ids: &std::collections::HashSet<&str>) -> bool {
    if relevant_ids.contains(group.id.as_str()) {
        return true;
    }
    group
        .groups
        .iter()
        .any(|child| group_is_relevant(child, relevant_ids))
}

/// Emit a `static GROUP_SCHEMA: &[GroupDef]` array that mirrors the MIG
/// segment-group hierarchy, **pruned to only include paths leading to groups
/// that have rules** (referenced in any PID's `group_rules`).
///
/// Pruning avoids trigger-tag conflicts between sibling groups: if SG2 and
/// SG5 both trigger on NAD at the root level but only SG5 leads to groups with
/// rules, SG2 is excluded from the schema so `group_segments_indexed`
/// correctly recognises the NAD that starts the rule-relevant subtree.
///
/// For profiles with no group_rules the schema is empty (`&[]`), and
/// `validate_lenient_grouped` short-circuits to pure flat validation.
fn emit_group_schema_static(out: &mut String, groups: &[MigGroup], ahb: &AhbProfile) {
    let relevant_ids = collect_relevant_group_ids(ahb);
    writeln!(out).unwrap();
    writeln!(out, "static GROUP_SCHEMA: &[GroupDef] = &[").unwrap();
    for group in groups {
        if group_is_relevant(group, &relevant_ids) {
            emit_group_def(out, group, 1, &relevant_ids);
        }
    }
    writeln!(out, "];").unwrap();
}

/// Recursively emit one `GroupDef` literal at the given indentation depth,
/// pruning children that are not relevant (have no rules and no relevant
/// descendants).
fn emit_group_def(
    out: &mut String,
    group: &MigGroup,
    depth: usize,
    relevant_ids: &std::collections::HashSet<&str>,
) {
    let pad = "    ".repeat(depth);
    let name = &group.id;
    let trigger = &group.trigger_segment;
    let relevant_children: Vec<&MigGroup> = group
        .groups
        .iter()
        .filter(|child| group_is_relevant(child, relevant_ids))
        .collect();
    if relevant_children.is_empty() {
        writeln!(
            out,
            "{pad}GroupDef {{ name: {name:?}, trigger: {trigger:?}, children: &[] }},"
        )
        .unwrap();
    } else {
        writeln!(out, "{pad}GroupDef {{").unwrap();
        writeln!(out, "{pad}    name: {name:?},").unwrap();
        writeln!(out, "{pad}    trigger: {trigger:?},").unwrap();
        writeln!(out, "{pad}    children: &[").unwrap();
        for child in relevant_children {
            emit_group_def(out, child, depth + 2, relevant_ids);
        }
        writeln!(out, "{pad}    ],").unwrap();
        writeln!(out, "{pad}}},").unwrap();
    }
}

fn emit_mig_rule_pack(out: &mut String, mig: &MigProfile) {
    let pack_name = format!("{}-MIG-{}", mig.message_type, mig.release);

    // Collect segments that must appear in EVERY valid message (de-dup by tag).
    // Segments inside *optional* groups are not globally mandatory.
    let globally_mandatory = collect_globally_mandatory_segments(mig);
    let mut seen_mandatory = std::collections::HashSet::new();
    let mandatory_segs: Vec<&MigSegment> = globally_mandatory
        .iter()
        .copied()
        .filter(|s| seen_mandatory.insert(s.tag.as_str()))
        .collect();

    // Collect HEADER-ONLY segments with max_occurrences > 1.
    //
    // F-010 fix: only emit global cardinality rules for segment tags that appear
    // EXCLUSIVELY in mig.segments (the top-level header) and NOT in any group.
    // Tags that appear in both the header and in groups (e.g. DTM in UTILMD)
    // cannot be correctly checked with a flat global count — any rule would
    // either produce false positives (header max applied globally) or false
    // negatives (group max applied globally).  Per-group cardinality is
    // enforced separately by the group-window rules below.
    let group_tags = collect_all_group_tags(&mig.segment_groups);
    let mut seen_card = std::collections::HashSet::new();
    let card_segs: Vec<&MigSegment> = mig
        .segments
        .iter()
        .filter(|s| {
            s.max_occurrences > 1
                && !group_tags.contains(s.tag.as_str())
                && seen_card.insert(s.tag.as_str())
        })
        .collect();

    // Collect top-level groups that have mandatory inner segments (for F-004 fix).
    // Only groups with mandatory non-trigger segments need a window rule.
    let groups_needing_window_rules: Vec<&MigGroup> = mig
        .segment_groups
        .iter()
        .filter(|g| {
            g.max_occurrences > 1
                && g.segments
                    .iter()
                    .any(|s| s.mandatory && s.tag != g.trigger_segment)
        })
        .collect();

    // F-001 fix: detect trigger tags that are shared among multiple top-level groups.
    //
    // When the same trigger tag (e.g. "NAD") is used by two or more top-level groups
    // (e.g. SG2 and SG5 in MSCONS), a flat segment count cannot distinguish which
    // occurrences belong to which group.  For such shared triggers:
    //
    //   max enforcement: emit a single combined-bounds rule that checks
    //     count(trigger) <= sum(max_occurrences for all top-level groups with
    //     that trigger) instead of an incorrect per-group rule.
    //
    //   min enforcement: emit a single combined-min rule that checks
    //     count(trigger) >= min(min_occurrences for all mandatory groups with
    //     that trigger).  This is conservative but avoids false positives.
    //
    // For triggers used by exactly one top-level group, the per-group rules are correct.
    let trigger_group_counts: std::collections::HashMap<&str, usize> = {
        let mut m: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for g in &mig.segment_groups {
            *m.entry(g.trigger_segment.as_str()).or_insert(0) += 1;
        }
        m
    };

    // Collect groups with a finite max_occurrences (F-007 fix: enforce group cardinality).
    // A group with max_occurrences == 1 is already handled by the segment-ordering rule
    // (trigger tag can only appear once), so we only emit explicit count rules for
    // groups that allow multiple occurrences.  Groups with max_occurrences == 0 are
    // treated as "no explicit limit" and skipped.
    // F-001: only include groups whose trigger is unique at this level.
    let groups_with_max: Vec<&MigGroup> = mig
        .segment_groups
        .iter()
        .filter(|g| {
            g.max_occurrences > 1
                && trigger_group_counts.get(g.trigger_segment.as_str()) == Some(&1)
        })
        .collect();

    // F-001: for shared triggers, collect combined max bounds (sum of all group maxes)
    // keyed by trigger tag.  Only emit when the combined bound is meaningfully finite.
    let combined_max_bounds: Vec<(&str, u32)> = {
        let mut by_trigger: std::collections::BTreeMap<&str, (u32, Vec<&str>)> =
            std::collections::BTreeMap::new();
        for g in &mig.segment_groups {
            if trigger_group_counts.get(g.trigger_segment.as_str()) == Some(&1) {
                continue; // unique trigger — handled by per-group rule above
            }
            let entry = by_trigger
                .entry(g.trigger_segment.as_str())
                .or_insert((0, Vec::new()));
            // Check for overflow when summing max_occurrences
            entry.0 = entry.0.saturating_add(g.max_occurrences);
            entry.1.push(g.id.as_str());
        }
        by_trigger
            .into_iter()
            .filter(|(_, (combined, _))| *combined > 1)
            .map(|(trigger, (combined, _))| (trigger, combined))
            .collect()
    };

    // Collect groups that require a minimum number of occurrences (F-006 fix).
    // Effective minimum = explicit `min_occurrences` field, or 1 for mandatory groups.
    // Skip groups with effective min == 0 (nothing to enforce).
    // F-001: only include groups whose trigger is unique at this level.
    let groups_with_min: Vec<(&MigGroup, u32)> = mig
        .segment_groups
        .iter()
        .filter_map(|g| {
            if trigger_group_counts.get(g.trigger_segment.as_str()) != Some(&1) {
                return None; // shared trigger — handled by combined rule
            }
            let effective_min = g.min_occurrences.unwrap_or(if g.mandatory { 1 } else { 0 });
            if effective_min > 0 {
                Some((g, effective_min))
            } else {
                None
            }
        })
        .collect();

    // F-001: for shared triggers, emit combined minimum checks (most restrictive mandatory group).
    let combined_min_bounds: Vec<(&str, u32)> = {
        let mut by_trigger: std::collections::BTreeMap<&str, u32> =
            std::collections::BTreeMap::new();
        for g in &mig.segment_groups {
            if trigger_group_counts.get(g.trigger_segment.as_str()) == Some(&1) {
                continue;
            }
            let effective_min = g.min_occurrences.unwrap_or(if g.mandatory { 1 } else { 0 });
            if effective_min > 0 {
                // Use the minimum across all groups sharing this trigger (conservative).
                let entry = by_trigger
                    .entry(g.trigger_segment.as_str())
                    .or_insert(effective_min);
                *entry = (*entry).min(effective_min);
            }
        }
        by_trigger.into_iter().collect()
    };

    // Emit mandatory segment rule functions
    for seg in &mandatory_segs {
        emit_mandatory_rule_fn(out, &seg.tag, &pack_name);
    }

    // Emit cardinality rule functions (header-only segments)
    for seg in &card_segs {
        emit_cardinality_rule_fn(out, &seg.tag, seg.max_occurrences, &pack_name);
    }

    // Emit group-window rule functions (conditional mandatory within groups)
    for group in &groups_needing_window_rules {
        emit_group_window_rule_fn(out, group, &pack_name);
    }

    // Emit group cardinality rule functions (F-007: enforce group max_occurrences)
    for group in &groups_with_max {
        emit_group_cardinality_rule_fn(out, group, &pack_name);
    }

    // Emit group minimum-occurrence rule functions (F-006: enforce mandatory group presence)
    for (group, min) in &groups_with_min {
        emit_group_min_occurrences_rule_fn(out, group, *min, &pack_name);
    }

    // F-001: emit combined-bounds rules for trigger tags shared by multiple top-level groups.
    for (trigger, combined_max) in &combined_max_bounds {
        emit_group_combined_max_rule_fn(out, trigger, *combined_max, &pack_name);
    }
    for (trigger, combined_min) in &combined_min_bounds {
        emit_group_combined_min_rule_fn(out, trigger, *combined_min, &pack_name);
    }

    // Emit segment-ordering rule (Layer 3.5)
    // F-002 fix: sequence now contains only top-level segments, preventing
    // false "out-of-order" violations in messages with repeating groups.
    let seq = mig_segment_sequence(mig);
    emit_segment_order_rule_fn(out, &seq, &pack_name);

    // Emit a LazyLock static that builds the MIG pack exactly once.
    let static_name = format!(
        "MIG_{}_PACK",
        mig.message_type.replace('-', "_").to_uppercase()
    );
    writeln!(out).unwrap();
    writeln!(
        out,
        "    static {static_name}: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {{"
    )
    .unwrap();
    writeln!(
        out,
        "        Arc::new(ProfileRulePack::new({:?})",
        pack_name
    )
    .unwrap();
    writeln!(out, "            .for_message_type({:?})", mig.message_type).unwrap();
    writeln!(out, "            .for_release({:?})", mig.release).unwrap();
    for seg in &mandatory_segs {
        let fn_name = mandatory_rule_fn_name(&seg.tag);
        writeln!(out, "            .with_stateless_rule_fn({fn_name})").unwrap();
    }
    for seg in &card_segs {
        let fn_name = cardinality_rule_fn_name(&seg.tag);
        writeln!(out, "            .with_stateless_rule_fn({fn_name})").unwrap();
    }
    for group in &groups_needing_window_rules {
        let fn_name = group_window_rule_fn_name(&group.trigger_segment);
        writeln!(out, "            .with_stateless_rule_fn({fn_name})").unwrap();
    }
    for group in &groups_with_max {
        let fn_name = group_cardinality_rule_fn_name(&group.trigger_segment, &group.id);
        writeln!(out, "            .with_stateless_rule_fn({fn_name})").unwrap();
    }
    for (group, _min) in &groups_with_min {
        let fn_name = group_min_occurrences_rule_fn_name(&group.trigger_segment, &group.id);
        writeln!(out, "            .with_stateless_rule_fn({fn_name})").unwrap();
    }
    for (trigger, _combined_max) in &combined_max_bounds {
        let fn_name = group_combined_max_rule_fn_name(trigger);
        writeln!(out, "            .with_stateless_rule_fn({fn_name})").unwrap();
    }
    for (trigger, _combined_min) in &combined_min_bounds {
        let fn_name = group_combined_min_rule_fn_name(trigger);
        writeln!(out, "            .with_stateless_rule_fn({fn_name})").unwrap();
    }
    writeln!(
        out,
        "            .with_stateless_rule_fn(rule_segment_order)"
    )
    .unwrap();
    writeln!(out, "        )").unwrap();
    writeln!(out, "    }});").unwrap();
    writeln!(out).unwrap();

    // Accessor: returns Arc::clone() — O(1), zero allocation (F-005 fix).
    writeln!(
        out,
        "    pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {{"
    )
    .unwrap();
    writeln!(out, "        Arc::clone(&{static_name})").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn mandatory_rule_fn_name(tag: &str) -> String {
    format!("rule_{}_mandatory", tag.to_lowercase())
}

/// Format an integer literal with `_` separators every three digits from the right.
///
/// Emitted into generated Rust source so that `clippy::unreadable_literal` does not fire.
/// Example: `100098` → `"100_098"`, `9999999` → `"9_999_999"`.
fn fmt_literal(n: u64) -> String {
    let s = n.to_string();
    let chars: Vec<u8> = s.bytes().collect();
    let len = chars.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push('_');
        }
        out.push(c as char);
    }
    out
}

fn cardinality_rule_fn_name(tag: &str) -> String {
    format!("rule_{}_max_occurrences", tag.to_lowercase())
}

fn emit_mandatory_rule_fn(out: &mut String, tag: &str, _pack_name: &str) {
    let fn_name = mandatory_rule_fn_name(tag);
    let rule_id = format!("MIG-{tag}-REQ");
    let msg = format!("mandatory segment {tag} is missing");
    writeln!(out).unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        if !segments.iter().any(|s| s.tag == {tag:?}) {{"
    )
    .unwrap();
    writeln!(out, "            issues.push(").unwrap();
    writeln!(
        out,
        "                ValidationIssue::new(ValidationSeverity::Error, {msg:?}.to_owned())"
    )
    .unwrap();
    writeln!(out, "                    .with_rule_id({rule_id:?})").unwrap();
    writeln!(out, "                    .with_segment({tag:?}.to_owned())").unwrap();
    writeln!(out, "            );").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn emit_cardinality_rule_fn(out: &mut String, tag: &str, max: u32, _pack_name: &str) {
    let fn_name = cardinality_rule_fn_name(tag);
    let rule_id = format!("MIG-{tag}-CARD-MAX");
    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Layer 3 — verify `{tag}` appears at most {max} times in the message header."
    )
    .unwrap();
    writeln!(out, "    ///").unwrap();
    writeln!(
        out,
        "    /// This rule only fires for segment tags that appear exclusively in the"
    )
    .unwrap();
    writeln!(
        out,
        "    /// message header (not in any segment group).  Tags shared between the"
    )
    .unwrap();
    writeln!(
        out,
        "    /// header and groups use per-group window rules instead (F-010 fix)."
    )
    .unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let count = segments.iter().filter(|s| s.tag == {tag:?}).count();"
    )
    .unwrap();
    let max_lit = fmt_literal(u64::from(max));
    writeln!(out, "        if count > {max_lit} {{").unwrap();
    writeln!(out, "            issues.push(").unwrap();
    writeln!(out, "                ValidationIssue::new(").unwrap();
    writeln!(out, "                    ValidationSeverity::Error,").unwrap();
    writeln!(
        out,
        "                    format!(\"segment {tag} occurs {{count}} times; maximum is {max_lit}\"),"
    )
    .unwrap();
    writeln!(out, "                )").unwrap();
    writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
    writeln!(out, "                .with_segment({tag:?}.to_owned())").unwrap();
    writeln!(out, "            );").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn group_window_rule_fn_name(trigger_tag: &str) -> String {
    format!("rule_group_{}_window", trigger_tag.to_lowercase())
}

fn group_cardinality_rule_fn_name(trigger_tag: &str, group_id: &str) -> String {
    let safe_id = group_id.to_lowercase().replace('-', "_");
    format!(
        "rule_group_{safe_id}_{}_max_occurrences",
        trigger_tag.to_lowercase()
    )
}

/// Emit a rule function that enforces the maximum number of group occurrences
/// for a given trigger segment (F-007 fix).
///
/// The generated function counts occurrences of `trigger_segment` in the flat
/// segment list.  Each occurrence marks the start of one group instance.  When
/// the count exceeds `max_occurrences`, an `Error`-severity issue is emitted.
fn emit_group_cardinality_rule_fn(out: &mut String, group: &MigGroup, pack_name: &str) {
    let trigger = &group.trigger_segment;
    let max = group.max_occurrences;
    let fn_name = group_cardinality_rule_fn_name(trigger, &group.id);
    let rule_id = format!("MIG-{pack_name}-GROUP-{}-{trigger}-CARD-MAX", group.id);

    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Layer 3 — verify the `{trigger}` segment group appears at most {max} times."
    )
    .unwrap();
    writeln!(out, "    ///").unwrap();
    writeln!(
        out,
        "    /// Each occurrence of the trigger segment `{trigger}` marks the start of"
    )
    .unwrap();
    writeln!(
        out,
        "    /// one group instance.  The MIG specifies a maximum of {max} instances."
    )
    .unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let count = segments.iter().filter(|s| s.tag == {trigger:?}).count();"
    )
    .unwrap();
    let max_lit = fmt_literal(u64::from(max));
    writeln!(out, "        if count > {max_lit} {{").unwrap();
    writeln!(out, "            issues.push(").unwrap();
    writeln!(out, "                ValidationIssue::new(").unwrap();
    writeln!(out, "                    ValidationSeverity::Error,").unwrap();
    writeln!(
        out,
        "                    format!(\"segment group triggered by {trigger} occurs {{count}} \
                        times; maximum is {max_lit}\"),"
    )
    .unwrap();
    writeln!(out, "                )").unwrap();
    writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
    writeln!(out, "                .with_segment({trigger:?}.to_owned())").unwrap();
    writeln!(out, "            );").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn group_min_occurrences_rule_fn_name(trigger_tag: &str, group_id: &str) -> String {
    let safe_id = group_id.to_lowercase().replace('-', "_");
    format!(
        "rule_group_{safe_id}_{}_min_occurrences",
        trigger_tag.to_lowercase()
    )
}

/// Emit a rule function that enforces the minimum number of group occurrences
/// for a given trigger segment (F-006 fix).
///
/// The generated function counts occurrences of `trigger_segment` in the flat
/// segment list.  When the count is below `min_occurrences`, an `Error`-severity
/// issue is emitted.
fn emit_group_min_occurrences_rule_fn(
    out: &mut String,
    group: &MigGroup,
    min: u32,
    pack_name: &str,
) {
    let trigger = &group.trigger_segment;
    let fn_name = group_min_occurrences_rule_fn_name(trigger, &group.id);
    let rule_id = format!("MIG-{pack_name}-GROUP-{}-{trigger}-CARD-MIN", group.id);

    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Layer 3 — verify the `{trigger}` segment group appears at least {min} time(s)."
    )
    .unwrap();
    writeln!(out, "    ///").unwrap();
    writeln!(
        out,
        "    /// The MIG specifies a minimum of {min} occurrence(s) for this group."
    )
    .unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let count = segments.iter().filter(|s| s.tag == {trigger:?}).count();"
    )
    .unwrap();
    writeln!(out, "        if count < {min} {{").unwrap();
    writeln!(out, "            issues.push(").unwrap();
    writeln!(out, "                ValidationIssue::new(").unwrap();
    writeln!(out, "                    ValidationSeverity::Error,").unwrap();
    writeln!(
        out,
        "                    format!(\"segment group triggered by {trigger} occurs {{count}} \
                        times; minimum is {min}\"),"
    )
    .unwrap();
    writeln!(out, "                )").unwrap();
    writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
    writeln!(out, "                .with_segment({trigger:?}.to_owned())").unwrap();
    writeln!(out, "            );").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn group_combined_max_rule_fn_name(trigger_tag: &str) -> String {
    format!(
        "rule_group_{}_combined_max_occurrences",
        trigger_tag.to_lowercase()
    )
}

fn group_combined_min_rule_fn_name(trigger_tag: &str) -> String {
    format!(
        "rule_group_{}_combined_min_occurrences",
        trigger_tag.to_lowercase()
    )
}

/// Emit a combined max-occurrences rule for a trigger tag shared by multiple top-level
/// groups (F-001 fix).
///
/// When multiple groups share the same trigger tag, individual per-group max rules
/// cannot correctly distinguish which occurrences belong to which group.  This function
/// emits a combined rule that checks the total occurrence count against the sum of all
/// group maxima — a necessary (though not sufficient) condition that is always correct.
fn emit_group_combined_max_rule_fn(
    out: &mut String,
    trigger: &str,
    combined_max: u32,
    pack_name: &str,
) {
    let fn_name = group_combined_max_rule_fn_name(trigger);
    let rule_id = format!("MIG-{pack_name}-{trigger}-COMBINED-CARD-MAX");

    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Layer 3 — combined max bound: all `{trigger}` groups together must not \
         exceed {combined_max} occurrences."
    )
    .unwrap();
    writeln!(out, "    ///").unwrap();
    writeln!(
        out,
        "    /// Multiple top-level groups share the trigger `{trigger}`.  A per-group flat"
    )
    .unwrap();
    writeln!(
        out,
        "    /// count would be ambiguous, so this rule enforces the combined upper bound"
    )
    .unwrap();
    writeln!(out, "    /// (sum of all group maxima) instead.").unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let count = segments.iter().filter(|s| s.tag == {trigger:?}).count();"
    )
    .unwrap();
    let combined_max_lit = fmt_literal(u64::from(combined_max));
    writeln!(out, "        if count > {combined_max_lit} {{").unwrap();
    writeln!(out, "            issues.push(").unwrap();
    writeln!(out, "                ValidationIssue::new(").unwrap();
    writeln!(out, "                    ValidationSeverity::Error,").unwrap();
    writeln!(
        out,
        "                    format!(\"segment groups triggered by {trigger} total {{count}} \
                        occurrences; combined maximum is {combined_max_lit}\"),"
    )
    .unwrap();
    writeln!(out, "                )").unwrap();
    writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
    writeln!(out, "                .with_segment({trigger:?}.to_owned())").unwrap();
    writeln!(out, "            );").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

/// Emit a combined min-occurrences rule for a trigger tag shared by multiple top-level
/// groups (F-001 fix).
fn emit_group_combined_min_rule_fn(
    out: &mut String,
    trigger: &str,
    combined_min: u32,
    pack_name: &str,
) {
    let fn_name = group_combined_min_rule_fn_name(trigger);
    let rule_id = format!("MIG-{pack_name}-{trigger}-COMBINED-CARD-MIN");

    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Layer 3 — combined min bound: `{trigger}` groups must appear at least \
         {combined_min} time(s) in total."
    )
    .unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        let count = segments.iter().filter(|s| s.tag == {trigger:?}).count();"
    )
    .unwrap();
    writeln!(out, "        if count < {combined_min} {{").unwrap();
    writeln!(out, "            issues.push(").unwrap();
    writeln!(out, "                ValidationIssue::new(").unwrap();
    writeln!(out, "                    ValidationSeverity::Error,").unwrap();
    writeln!(
        out,
        "                    format!(\"segment groups triggered by {trigger} total {{count}} \
                        occurrences; combined minimum is {combined_min}\"),"
    )
    .unwrap();
    writeln!(out, "                )").unwrap();
    writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
    writeln!(out, "                .with_segment({trigger:?}.to_owned())").unwrap();
    writeln!(out, "            );").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

/// Emit a group-window validation function for F-004.
///
/// The generated function scans the flat segment list for all occurrences of
/// the group trigger tag, builds per-window slices (trigger..next_trigger),
/// and validates mandatory segments within each window.
///
/// This catches "present-but-incomplete group" errors: when the group trigger
/// appears but a mandatory inner segment is absent.
fn emit_group_window_rule_fn(out: &mut String, group: &MigGroup, pack_name: &str) {
    let trigger = &group.trigger_segment;
    let fn_name = group_window_rule_fn_name(trigger);
    let rule_id = format!("MIG-{pack_name}-GROUP-{trigger}");

    // Collect mandatory segments within this group (excluding the trigger itself).
    let mandatory_inner: Vec<&str> = group
        .segments
        .iter()
        .filter(|s| s.mandatory && s.tag != *trigger)
        .map(|s| s.tag.as_str())
        .collect();

    if mandatory_inner.is_empty() {
        // Nothing to check — skip emitting this function entirely.
        // (The caller already filtered groups with mandatory inner segments,
        //  but guard here for safety.)
        return;
    }

    let mandatory_list: String = mandatory_inner
        .iter()
        .map(|t| format!("{t:?}"))
        .collect::<Vec<_>>()
        .join(", ");

    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Layer 3 — group-window rule for `{trigger}` groups (F-004)."
    )
    .unwrap();
    writeln!(out, "    ///").unwrap();
    writeln!(
        out,
        "    /// When a `{trigger}` group is present, the mandatory inner segments"
    )
    .unwrap();
    writeln!(
        out,
        "    /// must also be present within each group window."
    )
    .unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(
        out,
        "        const MANDATORY_INNER: &[&str] = &[{mandatory_list}];"
    )
    .unwrap();
    writeln!(out, "        // Find all positions of the trigger segment.").unwrap();
    writeln!(out, "        let trigger_positions: Vec<usize> = segments").unwrap();
    writeln!(out, "            .iter()").unwrap();
    writeln!(out, "            .enumerate()").unwrap();
    writeln!(out, "            .filter(|(_, s)| s.tag == {trigger:?})").unwrap();
    writeln!(out, "            .map(|(i, _)| i)").unwrap();
    writeln!(out, "            .collect();").unwrap();
    writeln!(
        out,
        "        for (win_idx, &start) in trigger_positions.iter().enumerate() {{"
    )
    .unwrap();
    writeln!(
        out,
        "            let end = trigger_positions.get(win_idx + 1).copied().unwrap_or(segments.len());"
    )
    .unwrap();
    writeln!(out, "            let window = &segments[start..end];").unwrap();
    writeln!(out, "            for &required_tag in MANDATORY_INNER {{").unwrap();
    writeln!(
        out,
        "                if !window.iter().any(|s| s.tag == required_tag) {{"
    )
    .unwrap();
    writeln!(out, "                    issues.push(").unwrap();
    writeln!(out, "                        ValidationIssue::new(").unwrap();
    writeln!(
        out,
        "                            ValidationSeverity::Error,"
    )
    .unwrap();
    writeln!(
        out,
        "                            format!(\"mandatory segment {{required_tag}} missing in {trigger} group at position {{start}}\"),"
    )
    .unwrap();
    writeln!(out, "                        )").unwrap();
    writeln!(out, "                        .with_rule_id({rule_id:?})").unwrap();
    writeln!(
        out,
        "                        .with_segment(required_tag.to_owned())"
    )
    .unwrap();
    writeln!(out, "                    );").unwrap();
    writeln!(out, "                }}").unwrap();
    writeln!(out, "            }}").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn emit_segment_order_rule_fn(out: &mut String, sequence: &[String], pack_name: &str) {
    let rule_id = format!("MIG-{pack_name}-ORDER");

    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Layer 3.5 — verify that segment tags appear in the normative sequence."
    )
    .unwrap();
    writeln!(out, "    ///").unwrap();
    writeln!(
        out,
        "    /// The rule does NOT require every tag to be present (that is Layer 3's job);"
    )
    .unwrap();
    writeln!(
        out,
        "    /// it only checks that tag positions are non-decreasing w.r.t. the expected order."
    )
    .unwrap();
    writeln!(out, "    fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{").unwrap();

    // Detect two-section messages: ordering_hint contains "UNS"
    if let Some(uns_pos) = sequence.iter().position(|s| s == "UNS") {
        // Split into header (before UNS) and detail (after UNS)
        let header: Vec<&str> = sequence[..uns_pos]
            .iter()
            .map(std::string::String::as_str)
            .collect();
        let detail: Vec<&str> = sequence[uns_pos + 1..]
            .iter()
            .map(std::string::String::as_str)
            .collect();

        let header_lit: String = header
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let detail_lit: String = detail
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ");

        writeln!(out, "        /// Header segment ordering (before UNS+D).").unwrap();
        writeln!(
            out,
            "        const EXPECTED_HEADER_ORDER: &[&str] = &[{header_lit}];"
        )
        .unwrap();
        writeln!(out, "        /// Detail segment ordering (after UNS+D).").unwrap();
        writeln!(
            out,
            "        const EXPECTED_DETAIL_ORDER: &[&str] = &[{detail_lit}];"
        )
        .unwrap();
        writeln!(out).unwrap();
        writeln!(
            out,
            "        /// Strict order check for the header section (no group repetition expected)."
        )
        .unwrap();
        writeln!(out, "        fn check_header_section(segs: &[edifact_rs::Segment<'_>], expected: &[&str], rule_id: &str, issues: &mut Vec<ValidationIssue>) {{").unwrap();
        writeln!(out, "            let mut cursor: usize = 0;").unwrap();
        writeln!(out, "            for seg in segs {{").unwrap();
        writeln!(out, "                if let Some(pos) = expected[cursor..].iter().position(|&t| t == seg.tag) {{").unwrap();
        writeln!(out, "                    cursor += pos;").unwrap();
        writeln!(
            out,
            "                }} else if expected.contains(&seg.tag) {{"
        )
        .unwrap();
        writeln!(out, "                    issues.push(").unwrap();
        writeln!(out, "                        ValidationIssue::new(").unwrap();
        writeln!(
            out,
            "                            ValidationSeverity::Error,"
        )
        .unwrap();
        writeln!(
            out,
            "                            \"segment appears out of order\".to_owned(),"
        )
        .unwrap();
        writeln!(out, "                        )").unwrap();
        writeln!(out, "                        .with_rule_id(rule_id)").unwrap();
        writeln!(
            out,
            "                        .with_segment(seg.tag.to_owned()),"
        )
        .unwrap();
        writeln!(out, "                    );").unwrap();
        writeln!(out, "                }}").unwrap();
        writeln!(out, "                // Unknown tags are passed through — they get caught by the DirectoryValidator.").unwrap();
        writeln!(out, "            }}").unwrap();
        writeln!(out, "        }}").unwrap();
        writeln!(out).unwrap();
        writeln!(
            out,
            "        /// Group-trigger-aware order check for the detail section (post-UNS)."
        )
        .unwrap();
        writeln!(out, "        ///").unwrap();
        writeln!(
            out,
            "        /// When the first tag in `expected` is seen again after the cursor has"
        )
        .unwrap();
        writeln!(
            out,
            "        /// already advanced, this indicates a new group-repetition occurrence"
        )
        .unwrap();
        writeln!(
            out,
            "        /// (e.g. a second `LOC` group in MSCONS).  The cursor is silently reset"
        )
        .unwrap();
        writeln!(
            out,
            "        /// to that position instead of reporting an ordering violation."
        )
        .unwrap();
        writeln!(out, "        fn check_detail_section(segs: &[edifact_rs::Segment<'_>], expected: &[&str], rule_id: &str, issues: &mut Vec<ValidationIssue>) {{").unwrap();
        writeln!(
            out,
            "            let group_trigger = expected.first().copied().unwrap_or(\"\");"
        )
        .unwrap();
        writeln!(out, "            let mut cursor: usize = 0;").unwrap();
        writeln!(out, "            for seg in segs {{").unwrap();
        writeln!(out, "                // A repeated group-trigger tag resets the cursor to allow multiple group occurrences.").unwrap();
        writeln!(
            out,
            "                if cursor > 0 && seg.tag == group_trigger {{"
        )
        .unwrap();
        writeln!(out, "                    cursor = 0;").unwrap();
        writeln!(out, "                }}").unwrap();
        writeln!(out, "                if let Some(pos) = expected[cursor..].iter().position(|&t| t == seg.tag) {{").unwrap();
        writeln!(out, "                    cursor += pos;").unwrap();
        writeln!(
            out,
            "                }} else if expected.contains(&seg.tag) {{"
        )
        .unwrap();
        writeln!(out, "                    issues.push(").unwrap();
        writeln!(out, "                        ValidationIssue::new(").unwrap();
        writeln!(
            out,
            "                            ValidationSeverity::Error,"
        )
        .unwrap();
        writeln!(
            out,
            "                            \"segment appears out of order\".to_owned(),"
        )
        .unwrap();
        writeln!(out, "                        )").unwrap();
        writeln!(out, "                        .with_rule_id(rule_id)").unwrap();
        writeln!(
            out,
            "                        .with_segment(seg.tag.to_owned()),"
        )
        .unwrap();
        writeln!(out, "                    );").unwrap();
        writeln!(out, "                }}").unwrap();
        writeln!(out, "                // Unknown tags are passed through — they get caught by the DirectoryValidator.").unwrap();
        writeln!(out, "            }}").unwrap();
        writeln!(out, "        }}").unwrap();
        writeln!(out).unwrap();
        writeln!(
            out,
            "        let uns_pos = segments.iter().position(|s| s.tag == \"UNS\");"
        )
        .unwrap();
        writeln!(
            out,
            "        let (header_segs, detail_segs) = match uns_pos {{"
        )
        .unwrap();
        writeln!(
            out,
            "            Some(pos) => (&segments[..pos], &segments[pos + 1..]),"
        )
        .unwrap();
        writeln!(out, "            None => (segments, &[][..]),").unwrap();
        writeln!(out, "        }};").unwrap();
        writeln!(
            out,
            "        check_header_section(header_segs, EXPECTED_HEADER_ORDER, {rule_id:?}, issues);"
        )
        .unwrap();
        writeln!(
            out,
            "        check_detail_section(detail_segs, EXPECTED_DETAIL_ORDER, {rule_id:?}, issues);"
        )
        .unwrap();
    } else {
        // Single-section message (no UNS separator)
        let seq_literal: String = sequence
            .iter()
            .map(|s| format!("{s:?}"))
            .collect::<Vec<_>>()
            .join(", ");

        writeln!(
            out,
            "        const EXPECTED_ORDER: &[&str] = &[{seq_literal}];"
        )
        .unwrap();
        writeln!(out, "        let mut cursor: usize = 0;").unwrap();
        writeln!(out, "        for seg in segments {{").unwrap();
        writeln!(out, "            if let Some(pos) = EXPECTED_ORDER[cursor..].iter().position(|&t| t == seg.tag) {{").unwrap();
        writeln!(out, "                cursor += pos;").unwrap();
        writeln!(
            out,
            "            }} else if EXPECTED_ORDER.contains(&seg.tag) {{"
        )
        .unwrap();
        writeln!(
            out,
            "                // Tag is known but already passed — ordering violation."
        )
        .unwrap();
        writeln!(out, "                issues.push(").unwrap();
        writeln!(out, "                    ValidationIssue::new(").unwrap();
        writeln!(out, "                        ValidationSeverity::Error,").unwrap();
        writeln!(
            out,
            "                        \"segment appears out of order\".to_owned(),"
        )
        .unwrap();
        writeln!(out, "                    )").unwrap();
        writeln!(out, "                    .with_rule_id({rule_id:?})").unwrap();
        writeln!(
            out,
            "                    .with_segment(seg.tag.to_owned()),"
        )
        .unwrap();
        writeln!(out, "                );").unwrap();
        writeln!(out, "            }}").unwrap();
        writeln!(out, "            // Unknown tags are passed through — they get caught by the DirectoryValidator.").unwrap();
        writeln!(out, "        }}").unwrap();
    }

    writeln!(out, "    }}").unwrap();
}

// ── AHB rule pack ─────────────────────────────────────────────────────────────

fn emit_ahb_rule_pack(out: &mut String, ahb: &AhbProfile, message_type: &str) {
    // Import AHB helper functions from the shared module instead of duplicating
    // them in every generated profile file (F-011).
    // `ahb_helpers.rs` is a hand-written, non-generated file that lives in
    // `src/generated/` alongside the generated profile modules.
    //
    // `#[allow(unused_imports)]` is required because not every profile uses all
    // helpers (e.g. a profile with no SOLL segments never calls `ahb_check_soll`),
    // and we import the full set to avoid per-profile dependency tracking in codegen.
    writeln!(out, "#[allow(unused_imports)]").unwrap();
    writeln!(out, "use super::ahb_helpers::{{").unwrap();
    writeln!(
        out,
        "    ahb_check_mandatory, ahb_check_soll, ahb_check_not_used,"
    )
    .unwrap();
    writeln!(out, "    ahb_check_qualifier, ahb_check_field_value,").unwrap();
    writeln!(
        out,
        "    ahb_check_required_qualifier, ahb_check_conditional,"
    )
    .unwrap();
    writeln!(out, "}};").unwrap();
    writeln!(out).unwrap();

    // ── Per-PID conditional rule functions ───────────────────────────────────
    //
    // Conditional rules have complex, per-rule logic and cannot easily be
    // expressed as closures calling a generic helper.  They are still emitted
    // as standalone functions.
    for pid_entry in &ahb.pruefidentifikatoren {
        emit_ahb_pid_conditional_rule_fns(out, pid_entry);
    }

    // ── Per-PID LazyLock packs ───────────────────────────────────────────────
    for pid_entry in &ahb.pruefidentifikatoren {
        emit_ahb_pid_rule_fn(out, pid_entry, message_type, &ahb.release);
    }

    // Emit the union-of-all-PIDs pack as a LazyLock so it is built only once
    // and reused on every subsequent call to ahb_rule_pack(None).
    let all_pack_name = format!("{}-AHB-{}-ALL", message_type, ahb.release);
    let release_str = &ahb.release;
    let lazy_name = format!(
        "AHB_ALL_PACK_{}",
        module_name(message_type, release_str).to_uppercase()
    );
    writeln!(out).unwrap();
    writeln!(
        out,
        "    static {lazy_name}: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {{"
    )
    .unwrap();
    if ahb.pruefidentifikatoren.is_empty() {
        writeln!(
            out,
            "        Arc::new(ProfileRulePack::new({all_pack_name:?}).for_message_type({message_type:?}).for_release({release_str:?}))"
        ).unwrap();
    } else {
        // Build the union-of-all-PIDs pack using merge_with_override (replaces removed .merge()).
        // ProfileRulePack: Clone is available in edifact-rs 0.9.
        writeln!(
            out,
            "        let pack = ProfileRulePack::new({all_pack_name:?}).for_message_type({message_type:?}).for_release({release_str:?});"
        ).unwrap();
        for pid_entry in &ahb.pruefidentifikatoren {
            let fn_name = ahb_pid_pack_fn_name(pid_entry.code);
            writeln!(out, "        let pack = pack.merge_with_override({fn_name}().as_ref().clone()).expect(\"AHB union pack merge_with_override failed\");").unwrap();
        }
        writeln!(out, "        Arc::new(pack)").unwrap();
    }
    writeln!(out, "    }});").unwrap();
    writeln!(out).unwrap();

    // Emit dispatch function
    writeln!(
        out,
        "    pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {{"
    )
    .unwrap();
    writeln!(
        out,
        "        match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {{"
    )
    .unwrap();
    for pid_entry in &ahb.pruefidentifikatoren {
        let fn_name = ahb_pid_pack_fn_name(pid_entry.code);
        writeln!(out, "            Some({}) => {fn_name}(),", pid_entry.code).unwrap();
    }
    // None => O(1) Arc::clone from the cached all-PIDs pack (zero allocation, F-005).
    writeln!(out, "            None => Arc::clone(&{lazy_name}),").unwrap();
    // Unknown PID: return a pack with one warning rule wrapped in Arc so validation
    // does not silently pass with zero checks.  The raw PID value is intentionally NOT
    // embedded in the message text (F-014: policy against including parsed data
    // in issue messages). The PID is available in the report's `pruefidentifikator` field.
    writeln!(
        out,
        "            Some(_unknown) => Arc::new(ProfileRulePack::new(\"unknown-pid\")"
    )
    .unwrap();
    writeln!(out, "                .for_message_type({message_type:?})").unwrap();
    writeln!(
        out,
        "                .with_named_stateless_rule_fn(\"AHB-UNKNOWN-PID\", |_segs, issues| {{"
    )
    .unwrap();
    writeln!(out, "                    issues.push(ValidationIssue::new(").unwrap();
    writeln!(out, "                        ValidationSeverity::Warning,").unwrap();
    writeln!(out, "                        \"Pruefidentifikator is not registered for this release \u{2014} AHB rules were not applied\",").unwrap();
    writeln!(
        out,
        "                    ).with_rule_id(\"AHB-UNKNOWN-PID\"));"
    )
    .unwrap();
    writeln!(out, "                }})),").unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
}

fn ahb_pid_pack_fn_name(code: u32) -> String {
    format!("ahb_{code}_pack")
}

fn emit_ahb_pid_conditional_rule_fns(out: &mut String, pid: &PruefidentifikatorEntry) {
    // Emit only the conditional rule functions (complex logic, kept as standalone fns).
    for rule in &pid.segment_rules {
        for (i, cond) in rule.conditional_rules.iter().enumerate() {
            match cond.operator {
                AhbOperator::X | AhbOperator::U | AhbOperator::O
                    if cond.secondary_tag.is_none() =>
                {
                    eprintln!(
                        "  gap: AHB PID {} segment {} cond {} operator={:?} requires \
                             secondary_tag — rule is emitted but will reference MISSING segment",
                        pid.code, rule.tag, i, cond.operator
                    );
                }
                _ => {}
            }
            emit_ahb_conditional_rule_fn(out, pid.code, &rule.tag, i, cond);
        }
    }
}

fn emit_ahb_pid_rule_fn(
    out: &mut String,
    pid: &PruefidentifikatorEntry,
    message_type: &str,
    release: &str,
) {
    let pack_name = format!("{}-AHB-{}-{}", message_type, release, pid.code);
    let fn_name = ahb_pid_pack_fn_name(pid.code);
    let static_name = format!("AHB_{}_PACK", pid.code);

    // Track how many times each segment tag has been seen so far (for multi-STS etc.)
    let mut tag_occurrence: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();

    // Emit a LazyLock<Arc<ProfileRulePack>> static that builds the per-PID pack exactly once.
    // Simple rules (mandatory, not-used, qualifier, field-value, required-qualifier) are
    // expressed as inline closures calling the module-level helper functions, reducing
    // generated code size significantly (F-002).
    writeln!(out).unwrap();
    writeln!(
        out,
        "    static {static_name}: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {{"
    )
    .unwrap();
    writeln!(out, "        Arc::new(ProfileRulePack::new({pack_name:?})").unwrap();
    writeln!(out, "            .for_message_type({message_type:?})").unwrap();
    writeln!(out, "            .for_release({release:?})").unwrap();

    for rule in &pid.segment_rules {
        let occ = *tag_occurrence.get(rule.tag.as_str()).unwrap_or(&0);
        tag_occurrence.insert(rule.tag.as_str(), occ + 1);
        let tag = &rule.tag;
        let code = pid.code;

        // Guard: warn about any requirement code the codegen does not explicitly handle.
        if !matches!(rule.requirement.as_str(), "M" | "S" | "C" | "N" | "O" | "X") {
            eprintln!(
                "  gap: unrecognised AHB requirement code {:?} in PID {} segment {} — \
                 no presence rule will be emitted; add a codegen branch if a new BDEW \
                 requirement code is introduced",
                rule.requirement, code, tag
            );
        }

        // Warn when requirement is C but no conditional_rules are provided.
        if rule.requirement == "C" && rule.conditional_rules.is_empty() {
            eprintln!(
                "  warning: AHB PID {} segment {} requirement=C but has no conditional_rules — \
                 add at least one ConditionalRule to enforce the BDEW Bedingungsoperator",
                code, tag
            );
        }

        // ── Mandatory presence (M) ──────────────────────────────────────────
        if rule.requirement == "M" {
            let rule_id = format!("AHB-{code}-{tag}-M");
            let msg = format!("mandatory segment {tag} is missing for Pruefidentifikator {code}");
            writeln!(
                out,
                "            .with_named_stateless_rule_fn({rule_id:?}, |segs, issues| {{"
            )
            .unwrap();
            writeln!(out, "                ahb_check_mandatory(segs, {tag:?}, {rule_id:?}, {msg:?}, \"{code}\", issues);").unwrap();
            writeln!(out, "            }})").unwrap();
        }

        // ── Soll presence (S) — warning if absent ──────────────────────────
        if rule.requirement == "S" {
            let rule_id = format!("AHB-{code}-{tag}-S");
            let msg =
                format!("segment {tag} should be present for Pruefidentifikator {code} (Soll)");
            writeln!(
                out,
                "            .with_named_stateless_rule_fn({rule_id:?}, |segs, issues| {{"
            )
            .unwrap();
            writeln!(out, "                ahb_check_soll(segs, {tag:?}, {rule_id:?}, {msg:?}, \"{code}\", issues);").unwrap();
            writeln!(out, "            }})").unwrap();
        }

        // ── Not-used (N, unconditional) ─────────────────────────────────────
        if rule.requirement == "N" && rule.conditional_rules.is_empty() {
            let rule_id = format!("AHB-{code}-{tag}-N");
            let msg =
                format!("segment {tag} must not appear for Pruefidentifikator {code} (not used)");
            writeln!(
                out,
                "            .with_named_stateless_rule_fn({rule_id:?}, |segs, issues| {{"
            )
            .unwrap();
            writeln!(out, "                ahb_check_not_used(segs, {tag:?}, {rule_id:?}, {msg:?}, \"{code}\", issues);").unwrap();
            writeln!(out, "            }})").unwrap();
        }

        // ── Conditional rules ───────────────────────────────────────────────
        // These stay as standalone functions (complex logic — see conditional fns above).
        for (i, _cond) in rule.conditional_rules.iter().enumerate() {
            let rfn = ahb_conditional_rule_fn_name(code, tag, i);
            writeln!(out, "            .with_stateless_rule_fn({rfn})").unwrap();
        }

        // ── Qualifier restrictions ──────────────────────────────────────────
        for (de_id, allowed) in &rule.qualifier_restrictions {
            if !allowed.is_empty() {
                let rule_id = format!("AHB-{code}-{tag}-{de_id}-Q");
                let allowed_display: String = allowed
                    .iter()
                    .map(|v| format!("'{v}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let msg = format!(
                    "segment {tag} DE {de_id} (element 0, component 0): qualifier is not one of the allowed values [{allowed_display}]"
                );
                let allowed_literal: String = allowed
                    .iter()
                    .map(|v| format!("{v:?}"))
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(
                    out,
                    "            .with_named_stateless_rule_fn({rule_id:?}, |segs, issues| {{"
                )
                .unwrap();
                writeln!(out, "                ahb_check_qualifier(segs, {tag:?}, {rule_id:?}, {msg:?}, |q| matches!(q, {allowed_literal}), \"{code}\", issues);").unwrap();
                writeln!(out, "            }})").unwrap();
            }
        }

        // ── Field-value rules ───────────────────────────────────────────────
        for fr in &rule.field_rules {
            if !fr.allowed_values.is_empty() {
                let rule_id = format!("AHB-{code}-{tag}-{}-V", fr.element);
                let ei = fr.element_index;
                let allowed_display: String = fr
                    .allowed_values
                    .iter()
                    .map(|v| format!("'{v}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let msg = format!(
                    "segment {tag} DE {} (element {ei}, component 0): value is not one of the allowed values [{allowed_display}]",
                    fr.element
                );
                let allowed_literal: String = fr
                    .allowed_values
                    .iter()
                    .map(|v| format!("{v:?}"))
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(
                    out,
                    "            .with_named_stateless_rule_fn({rule_id:?}, |segs, issues| {{"
                )
                .unwrap();
                writeln!(out, "                ahb_check_field_value(segs, {tag:?}, {ei}, {rule_id:?}, {msg:?}, |v| matches!(v, {allowed_literal}), \"{code}\", issues);").unwrap();
                writeln!(out, "            }})").unwrap();
            }
        }

        // ── Required-qualifier rules ────────────────────────────────────────
        for (de_id, required_vals) in &rule.required_qualifiers {
            if !required_vals.is_empty() {
                let rule_id = format!("AHB-{code}-{tag}-{de_id}-RQ");
                let required_display: String = required_vals
                    .iter()
                    .map(|v| format!("'{v}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let msg = format!(
                    "mandatory segment {tag} with DE {de_id} qualifier {required_display} is missing"
                );
                let required_literal: String = required_vals
                    .iter()
                    .map(|v| format!("{v:?}"))
                    .collect::<Vec<_>>()
                    .join(" | ");
                writeln!(
                    out,
                    "            .with_named_stateless_rule_fn({rule_id:?}, |segs, issues| {{"
                )
                .unwrap();
                writeln!(out, "                ahb_check_required_qualifier(segs, {tag:?}, {rule_id:?}, {msg:?}, |q| matches!(q, {required_literal}), \"{code}\", issues);").unwrap();
                writeln!(out, "            }})").unwrap();
            }
        }
    }

    // ── Group-instance-scoped rules (F-001) ─────────────────────────────────
    // These are evaluated once per occurrence of the named segment-group (e.g.
    // once per SG4 instance) rather than over the flat message segment list.
    // `group_scope` accepts any Into<Arc<str>> since edifact-rs 0.10.0.
    for gr in &pid.group_rules {
        emit_ahb_group_rule(out, pid.code, gr);
    }

    // F-021: cap issues per rule to prevent report flooding from a single broken file.
    writeln!(out, "            .with_max_issues_per_rule(50)").unwrap();
    writeln!(out, "        )").unwrap();
    writeln!(out, "    }});").unwrap();
    writeln!(out).unwrap();

    // Accessor: returns Arc::clone() — O(1), zero allocation (F-005 fix).
    writeln!(out, "    fn {fn_name}() -> Arc<ProfileRulePack> {{").unwrap();
    writeln!(out, "        Arc::clone(&{static_name})").unwrap();
    writeln!(out, "    }}").unwrap();
}

fn ahb_conditional_rule_fn_name(pid: u32, tag: &str, idx: usize) -> String {
    format!("rule_ahb_{pid}_{}_cond_{idx}", tag.to_lowercase())
}

/// Emit one group-scoped rule for the AHB pack builder chain.
///
/// Delegates to the built-in `require_segment_in_group` / `forbid_segment_in_group`
/// helpers for M/N requirements, emits custom `with_scoped_group_rule_fn` closures
/// for qualifier-restriction checks, and emits inlined group-scoped conditional
/// closures for `conditional_rules` entries (F-001 fix).
fn emit_ahb_group_rule(out: &mut String, code: u32, gr: &AhbGroupRule) {
    let group_id = &gr.group_id;
    let tag = &gr.tag;

    // ── Mandatory presence in group ─────────────────────────────────────────
    if gr.requirement == "M" {
        let rule_id = format!("AHB-{code}-{group_id}-{tag}-M");
        writeln!(
            out,
            "            .require_segment_in_group({group_id:?}, {tag:?}, {rule_id:?})"
        )
        .unwrap();
    }

    // ── Not-used in group ───────────────────────────────────────────────────
    if gr.requirement == "N" {
        let rule_id = format!("AHB-{code}-{group_id}-{tag}-N");
        writeln!(
            out,
            "            .forbid_segment_in_group({group_id:?}, {tag:?}, {rule_id:?})"
        )
        .unwrap();
    }

    // ── Qualifier restrictions within group ─────────────────────────────────
    for (de_id, allowed) in &gr.qualifier_restrictions {
        if !allowed.is_empty() {
            let rule_id = format!("AHB-{code}-{group_id}-{tag}-{de_id}-Q");
            let allowed_display: String = allowed
                .iter()
                .map(|v| format!("'{v}'"))
                .collect::<Vec<_>>()
                .join(", ");
            let msg = format!(
                "in group {group_id}: segment {tag} DE {de_id} qualifier is not one of [{allowed_display}]"
            );
            let allowed_literal: String = allowed
                .iter()
                .map(|v| format!("{v:?}"))
                .collect::<Vec<_>>()
                .join(" | ");
            // group_scope accepts any Into<Arc<str>> since edifact-rs 0.10.0.
            // The closure signature is |group, segs, ctx, issues| where `segs` is
            // *already* the per-SG-instance sub-slice (edifact-rs slices via
            // `all_segments[group.total_span.clone()]` before invoking the closure).
            // No further slicing is needed — just use `segs` directly (F-001/F-022 fix).
            writeln!(
                out,
                "            .with_scoped_group_rule_fn({group_id:?}, {rule_id:?}, |group, segs, _ctx, issues| {{"
            )
            .unwrap();
            writeln!(out, "                let __gs_start = issues.len();").unwrap();
            writeln!(
                out,
                "                ahb_check_qualifier(segs, {tag:?}, {rule_id:?}, {msg:?}, |q| matches!(q, {allowed_literal}), \"{code}\", issues);"
            )
            .unwrap();
            writeln!(
                out,
                "                for __gi in &mut issues[__gs_start..] {{"
            )
            .unwrap();
            writeln!(out, "                    __gi.context.push((\"group_occurrence\".to_owned(), group.occurrence_index.to_string()));").unwrap();
            writeln!(out, "                }}").unwrap();
            writeln!(out, "            }})").unwrap();
        }
    }

    // ── Intra-SG conditional rules (F-001) ──────────────────────────────────
    // Each rule is emitted as a `with_scoped_group_rule_fn` closure.  Since
    // edifact-rs passes `segs` as the per-instance sub-slice (via `total_span`),
    // the condition and consequence are evaluated only against segments within
    // the current group occurrence — exactly as the BDEW AHB intends.
    for (i, cond) in gr.conditional_rules.iter().enumerate() {
        emit_ahb_group_conditional_closure(out, code, group_id, tag, i, cond);
    }
}

/// Emit a `with_scoped_group_rule_fn` closure for one intra-SG conditional rule.
///
/// This is the group-scoped counterpart of `emit_ahb_conditional_rule_fn`.
/// The key difference: the rule body uses `segs` (which is already the
/// per-instance sub-slice provided by edifact-rs `walk_group_tree`) so all
/// segment searches are naturally confined to the current group occurrence.
///
/// Rule IDs use the format `AHB-{pid}-{group_id}-{tag}-{op}{idx}` to avoid
/// collisions with flat-scope rule IDs and to make diagnostics clearly
/// group-qualified.
fn emit_ahb_group_conditional_closure(
    out: &mut String,
    pid: u32,
    group_id: &str,
    tag: &str,
    idx: usize,
    rule: &AhbConditionalRule,
) {
    let op_label = match rule.operator {
        AhbOperator::I => "I",
        AhbOperator::V => "V",
        AhbOperator::E => "E",
        AhbOperator::X => "X",
        AhbOperator::U => "U",
        AhbOperator::O => "O",
        AhbOperator::G => "G",
        AhbOperator::K => "K",
        AhbOperator::Z => "Z",
    };
    let rule_id = format!("AHB-{pid}-{group_id}-{tag}-{op_label}{idx}");
    let when_tag = &rule.when_tag;
    let when_idx = rule.when_element_index;
    let effective_absent = rule.operator == AhbOperator::V;

    // Build primary trigger value check
    let primary_check = if !rule.when_value_alternatives.is_empty() {
        let arms: Vec<String> = rule
            .when_value_alternatives
            .iter()
            .map(|v| format!("v == {v:?}"))
            .collect();
        format!(
            " && s.element_str({when_idx}).is_some_and(|v| {})",
            arms.join(" || ")
        )
    } else if let Some(ref val) = rule.when_value {
        format!(" && s.element_str({when_idx}).is_some_and(|v| v == {val:?})")
    } else {
        String::new()
    };

    // Build additional AND-semantics element checks
    let additional_checks: String = rule
        .when_additional_elements
        .iter()
        .map(|e| {
            let eidx = e.element_index;
            if !e.value_alternatives.is_empty() {
                let arms: Vec<String> = e
                    .value_alternatives
                    .iter()
                    .map(|v| format!("v == {v:?}"))
                    .collect();
                format!(
                    " && s.element_str({eidx}).is_some_and(|v| {})",
                    arms.join(" || ")
                )
            } else if let Some(ref v) = e.value {
                format!(" && s.element_str({eidx}).is_some_and(|v| v == {v:?})")
            } else {
                format!(" && s.element_str({eidx}).is_some()")
            }
        })
        .collect();

    // Human-readable trigger description (for error messages)
    let primary_val_desc = if !rule.when_value_alternatives.is_empty() {
        let alts = rule.when_value_alternatives.join("|");
        format!("DE[{when_idx}]∈{{{alts}}}")
    } else if let Some(ref val) = rule.when_value {
        format!("DE[{when_idx}]={val:?}")
    } else {
        String::new()
    };
    let extra_desc: String = rule
        .when_additional_elements
        .iter()
        .map(|e| {
            let idx = e.element_index;
            if !e.value_alternatives.is_empty() {
                let alts = e.value_alternatives.join("|");
                format!("+DE[{idx}]∈{{{alts}}}")
            } else if let Some(ref v) = e.value {
                format!("+DE[{idx}]={v:?}")
            } else {
                format!("+DE[{idx}]=*")
            }
        })
        .collect();
    let trigger_desc = if primary_val_desc.is_empty() && extra_desc.is_empty() {
        when_tag.to_string()
    } else {
        format!("{when_tag} {primary_val_desc}{extra_desc}")
    };
    let condition_desc = match rule.operator {
        AhbOperator::X => {
            let b = rule.secondary_tag.as_deref().unwrap_or("?");
            format!("XOR: exactly one of {{{when_tag}, {b}}} must appear in {group_id}")
        }
        AhbOperator::U => {
            let b = rule.secondary_tag.as_deref().unwrap_or("?");
            format!("AND: both {when_tag} and {b} must appear in {group_id}")
        }
        AhbOperator::O => {
            let b = rule.secondary_tag.as_deref().unwrap_or("?");
            format!("OR: at least one of {{{when_tag}, {b}}} must appear in {group_id}")
        }
        AhbOperator::G => {
            let n = rule.count_threshold;
            format!("G: when {trigger_desc} appears more than {n} time(s) in {group_id}")
        }
        AhbOperator::K => {
            let mut all_tags: Vec<&str> = vec![when_tag.as_str()];
            if let Some(ref b) = rule.secondary_tag {
                all_tags.push(b.as_str());
            }
            for t in &rule.additional_tags {
                all_tags.push(t.as_str());
            }
            let tags_list = all_tags.join(", ");
            format!("K: at most one of {{{tags_list}}} may appear in {group_id}")
        }
        _ => {
            if effective_absent {
                format!("V: when {trigger_desc} is absent in {group_id}")
            } else {
                format!("{op_label}: when {trigger_desc} is present in {group_id}")
            }
        }
    };

    let description_comment = if rule.description.is_empty() {
        String::new()
    } else {
        format!(" // {}", rule.description)
    };

    // Presence expression for trigger segment A using per-instance `segs`
    let a_present_expr =
        format!("segs.iter().any(|s| s.tag == {when_tag:?}{primary_check}{additional_checks})");

    writeln!(out).unwrap();
    writeln!(
        out,
        "            // Bedingungsoperator {op_label} — {condition_desc}{description_comment}"
    )
    .unwrap();
    writeln!(
        out,
        "            .with_scoped_group_rule_fn({group_id:?}, {rule_id:?}, |group, segs, _ctx, issues| {{"
    )
    .unwrap();
    writeln!(out, "                let __gs_start = issues.len();").unwrap();

    match rule.operator {
        // ── X — Exclusive-OR: exactly one of {A, B} ─────────────────────────
        AhbOperator::X => {
            let b_tag = rule.secondary_tag.as_deref().unwrap_or("MISSING");
            let msg = format!(
                "in {group_id}: exactly one of {{{when_tag}, {b_tag}}} must appear for Pruefidentifikator {pid} ({condition_desc})"
            );
            writeln!(out, "                let __a = segs.iter().any(|s| s.tag == {when_tag:?}{primary_check}{additional_checks});").unwrap();
            writeln!(
                out,
                "                let __b = segs.iter().any(|s| s.tag == {b_tag:?});"
            )
            .unwrap();
            writeln!(out, "                if !(__a ^ __b) {{").unwrap();
            writeln!(out, "                    issues.push(ValidationIssue::new(ValidationSeverity::Error, {msg:?}.to_owned()).with_rule_id({rule_id:?}));").unwrap();
            writeln!(out, "                }}").unwrap();
        }
        // ── U — AND: both A and B must appear ───────────────────────────────
        AhbOperator::U => {
            let b_tag = rule.secondary_tag.as_deref().unwrap_or("MISSING");
            let msg = format!(
                "in {group_id}: both {when_tag} and {b_tag} must appear for Pruefidentifikator {pid} ({condition_desc})"
            );
            writeln!(out, "                let __a = segs.iter().any(|s| s.tag == {when_tag:?}{primary_check}{additional_checks});").unwrap();
            writeln!(
                out,
                "                let __b = segs.iter().any(|s| s.tag == {b_tag:?});"
            )
            .unwrap();
            writeln!(out, "                if !__a || !__b {{").unwrap();
            writeln!(out, "                    issues.push(ValidationIssue::new(ValidationSeverity::Error, {msg:?}.to_owned()).with_rule_id({rule_id:?}));").unwrap();
            writeln!(out, "                }}").unwrap();
        }
        // ── O — OR: at least one of {A, B} ──────────────────────────────────
        AhbOperator::O => {
            let b_tag = rule.secondary_tag.as_deref().unwrap_or("MISSING");
            let msg = format!(
                "in {group_id}: at least one of {{{when_tag}, {b_tag}}} must appear for Pruefidentifikator {pid} ({condition_desc})"
            );
            writeln!(out, "                let __a = segs.iter().any(|s| s.tag == {when_tag:?}{primary_check}{additional_checks});").unwrap();
            writeln!(
                out,
                "                let __b = segs.iter().any(|s| s.tag == {b_tag:?});"
            )
            .unwrap();
            writeln!(out, "                if !__a && !__b {{").unwrap();
            writeln!(out, "                    issues.push(ValidationIssue::new(ValidationSeverity::Error, {msg:?}.to_owned()).with_rule_id({rule_id:?}));").unwrap();
            writeln!(out, "                }}").unwrap();
        }
        // ── K — at most one of {A, B, ...} may appear ───────────────────────
        AhbOperator::K => {
            let mut all_tags: Vec<&str> = vec![when_tag.as_str()];
            if let Some(ref b) = rule.secondary_tag {
                all_tags.push(b.as_str());
            }
            for t in &rule.additional_tags {
                all_tags.push(t.as_str());
            }
            let tags_display = all_tags.join(", ");
            let msg = format!(
                "in {group_id}: at most one of {{{tags_display}}} may appear for Pruefidentifikator {pid} ({condition_desc})"
            );
            for (i, t) in all_tags.iter().enumerate() {
                writeln!(
                    out,
                    "                let __k{i} = segs.iter().any(|s| s.tag == {t:?});"
                )
                .unwrap();
            }
            let sum_expr: String = (0..all_tags.len())
                .map(|i| format!("__k{i} as usize"))
                .collect::<Vec<_>>()
                .join(" + ");
            writeln!(out, "                if {sum_expr} > 1 {{").unwrap();
            writeln!(out, "                    issues.push(ValidationIssue::new(ValidationSeverity::Error, {msg:?}.to_owned()).with_rule_id({rule_id:?}));").unwrap();
            writeln!(out, "                }}").unwrap();
        }
        // ── G — count-threshold gate ─────────────────────────────────────────
        AhbOperator::G => {
            let n = rule.count_threshold;
            let condition_holds_expr = format!(
                "segs.iter().filter(|s| s.tag == {when_tag:?}{primary_check}{additional_checks}).count() > {n}"
            );
            emit_group_conditional_consequence(
                out,
                tag,
                pid,
                group_id,
                rule,
                &rule_id,
                &condition_holds_expr,
                &condition_desc,
            );
        }
        // ── I / V / E / Z — implication, absent-trigger, exclusion ───────────
        _ => {
            let condition_holds_expr = if effective_absent {
                format!("!{a_present_expr}")
            } else {
                a_present_expr.clone()
            };
            emit_group_conditional_consequence(
                out,
                tag,
                pid,
                group_id,
                rule,
                &rule_id,
                &condition_holds_expr,
                &condition_desc,
            );
        }
    }

    writeln!(
        out,
        "                for __gi in &mut issues[__gs_start..] {{"
    )
    .unwrap();
    writeln!(out, "                    __gi.context.push((\"group_occurrence\".to_owned(), group.occurrence_index.to_string()));").unwrap();
    writeln!(out, "                }}").unwrap();
    writeln!(out, "            }})").unwrap();
}

/// Emit the consequence block for a group-scoped conditional closure.
///
/// Mirrors `emit_conditional_consequence` but uses `segs` (per-instance) instead
/// of `segments` (flat), and includes `{group_id}` in error messages.
fn emit_group_conditional_consequence(
    out: &mut String,
    tag: &str,
    pid: u32,
    group_id: &str,
    rule: &AhbConditionalRule,
    rule_id: &str,
    condition_holds_expr: &str,
    condition_desc: &str,
) {
    if rule.then_requirement == "N" {
        let msg = format!(
            "in {group_id}: segment {tag} must not appear for Pruefidentifikator {pid} ({condition_desc})"
        );
        writeln!(out, "                if {condition_holds_expr} {{").unwrap();
        writeln!(
            out,
            "                    if segs.iter().any(|s| s.tag == {tag:?}) {{"
        )
        .unwrap();
        writeln!(out, "                        issues.push(ValidationIssue::new(ValidationSeverity::Error, {msg:?}.to_owned()).with_rule_id({rule_id:?}).with_segment({tag:?}.to_owned()));").unwrap();
        writeln!(out, "                    }}").unwrap();
        writeln!(out, "                }}").unwrap();
    } else {
        let severity = if rule.then_requirement == "S" {
            "ValidationSeverity::Warning"
        } else {
            "ValidationSeverity::Error"
        };
        let presence_check_expr = if let Some(ref qval) = rule.then_qualifier_value {
            let qidx = rule.then_qualifier_index;
            format!(
                "!segs.iter().any(|s| s.tag == {tag:?} && s.element_str({qidx}).is_some_and(|v| v == {qval:?}))"
            )
        } else {
            format!("!segs.iter().any(|s| s.tag == {tag:?})")
        };
        let obligation = if rule.then_requirement == "S" {
            "should be present"
        } else {
            "is missing"
        };
        let msg = if let Some(ref qval) = rule.then_qualifier_value {
            let qidx = rule.then_qualifier_index;
            format!(
                "in {group_id}: conditional segment {tag} (DE[{qidx}]={qval:?}) {obligation} for Pruefidentifikator {pid} ({condition_desc})"
            )
        } else {
            format!(
                "in {group_id}: conditional segment {tag} {obligation} for Pruefidentifikator {pid} ({condition_desc})"
            )
        };
        writeln!(
            out,
            "                if {condition_holds_expr} && {presence_check_expr} {{"
        )
        .unwrap();
        writeln!(out, "                    issues.push(ValidationIssue::new({severity}, {msg:?}.to_owned()).with_rule_id({rule_id:?}).with_segment({tag:?}.to_owned()));").unwrap();
        writeln!(out, "                }}").unwrap();
    }
}

/// Emit a conditional rule function for the `idx`-th `ConditionalRule` on `tag` in `pid`.
///
/// Handles all nine BDEW Bedingungsoperator categories (I, V, E, X, U, O, G, Z).
fn emit_ahb_conditional_rule_fn(
    out: &mut String,
    pid: u32,
    tag: &str,
    idx: usize,
    rule: &AhbConditionalRule,
) {
    let fn_name = ahb_conditional_rule_fn_name(pid, tag, idx);
    let op_label = match rule.operator {
        AhbOperator::I => "I",
        AhbOperator::V => "V",
        AhbOperator::E => "E",
        AhbOperator::X => "X",
        AhbOperator::U => "U",
        AhbOperator::O => "O",
        AhbOperator::G => "G",
        AhbOperator::K => "K",
        AhbOperator::Z => "Z",
    };
    let rule_id = format!("AHB-{pid}-{tag}-{op_label}{idx}");
    let when_tag = &rule.when_tag;
    let when_idx = rule.when_element_index;

    // Build human-readable trigger description
    let primary_val_desc = if !rule.when_value_alternatives.is_empty() {
        let alts = rule.when_value_alternatives.join("|");
        format!("DE[{when_idx}]∈{{{alts}}}")
    } else if let Some(ref val) = rule.when_value {
        format!("DE[{when_idx}]={val:?}")
    } else {
        String::new()
    };

    let extra_desc: String = rule
        .when_additional_elements
        .iter()
        .map(|e| {
            let idx = e.element_index;
            if !e.value_alternatives.is_empty() {
                let alts = e.value_alternatives.join("|");
                format!("+DE[{idx}]∈{{{alts}}}")
            } else if let Some(ref v) = e.value {
                format!("+DE[{idx}]={v:?}")
            } else {
                format!("+DE[{idx}]=*")
            }
        })
        .collect();

    // Effective operator V = fires when trigger is ABSENT.
    let effective_absent = rule.operator == AhbOperator::V;

    let trigger_base_desc = if primary_val_desc.is_empty() && extra_desc.is_empty() {
        when_tag.to_string()
    } else {
        format!("{when_tag} {primary_val_desc}{extra_desc}")
    };

    let condition_desc = match rule.operator {
        AhbOperator::X => {
            let b = rule.secondary_tag.as_deref().unwrap_or("?");
            format!("XOR: exactly one of {{{when_tag}, {b}}} must appear")
        }
        AhbOperator::U => {
            let b = rule.secondary_tag.as_deref().unwrap_or("?");
            format!("AND: both {when_tag} and {b} must appear")
        }
        AhbOperator::O => {
            let b = rule.secondary_tag.as_deref().unwrap_or("?");
            format!("OR: at least one of {{{when_tag}, {b}}} must appear")
        }
        AhbOperator::G => {
            let n = rule.count_threshold;
            format!("G: when {trigger_base_desc} appears more than {n} time(s)")
        }
        AhbOperator::K => {
            let mut all_tags: Vec<&str> = vec![when_tag.as_str()];
            if let Some(ref b) = rule.secondary_tag {
                all_tags.push(b.as_str());
            }
            for t in &rule.additional_tags {
                all_tags.push(t.as_str());
            }
            let tags_list = all_tags.join(", ");
            format!("K: at most one of {{{tags_list}}} may appear")
        }
        _ => {
            if effective_absent {
                format!("V: when {trigger_base_desc} is absent")
            } else {
                format!("{op_label}: when {trigger_base_desc} is present")
            }
        }
    };

    let description_comment = if rule.description.is_empty() {
        String::new()
    } else {
        format!(" // {}", rule.description)
    };

    writeln!(out).unwrap();
    writeln!(
        out,
        "    /// Bedingungsoperator {op_label} — {condition_desc}{description_comment}"
    )
    .unwrap();
    writeln!(
        out,
        "    fn {fn_name}(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {{"
    )
    .unwrap();
    writeln!(out, "        let __start = issues.len();").unwrap();

    // Build the primary value-check sub-expression for the trigger segment A
    let primary_check = if !rule.when_value_alternatives.is_empty() {
        let arms: Vec<String> = rule
            .when_value_alternatives
            .iter()
            .map(|v| format!("v == {v:?}"))
            .collect();
        let arms_expr = arms.join(" || ");
        format!(" && s.element_str({when_idx}).is_some_and(|v| {arms_expr})")
    } else if let Some(ref val) = rule.when_value {
        format!(" && s.element_str({when_idx}).is_some_and(|v| v == {val:?})")
    } else {
        String::new()
    };

    // Build additional AND-semantics element checks on trigger segment A
    let additional_checks: String = rule
        .when_additional_elements
        .iter()
        .map(|e| {
            let eidx = e.element_index;
            if !e.value_alternatives.is_empty() {
                let arms: Vec<String> = e
                    .value_alternatives
                    .iter()
                    .map(|v| format!("v == {v:?}"))
                    .collect();
                let arms_expr = arms.join(" || ");
                format!(" && s.element_str({eidx}).is_some_and(|v| {arms_expr})")
            } else if let Some(ref v) = e.value {
                format!(" && s.element_str({eidx}).is_some_and(|v| v == {v:?})")
            } else {
                format!(" && s.element_str({eidx}).is_some()")
            }
        })
        .collect();

    // Presence expression for trigger segment A (used by I/V/E/G/Z)
    let a_present_expr =
        format!("segments.iter().any(|s| s.tag == {when_tag:?}{primary_check}{additional_checks})");

    match rule.operator {
        // ── X — Exclusive-OR: exactly one of {A, B} must appear ──────────────
        AhbOperator::X => {
            let b_tag = rule
                .secondary_tag
                .as_deref()
                .unwrap_or_else(|| {
                    eprintln!(
                        "  error: AHB PID {pid} segment {tag} cond {idx} operator=X missing secondary_tag"
                    );
                    "MISSING"
                });
            let msg = format!(
                "operator X violation for Pruefidentifikator {pid}: exactly one of \
                 {{{when_tag}, {b_tag}}} must appear, but got both or neither"
            );
            writeln!(out, "        let a_present = {a_present_expr};").unwrap();
            writeln!(
                out,
                "        let b_present = segments.iter().any(|s| s.tag == {b_tag:?});"
            )
            .unwrap();
            writeln!(out, "        if a_present == b_present {{").unwrap();
            writeln!(out, "            issues.push(").unwrap();
            writeln!(out, "                ValidationIssue::new(").unwrap();
            writeln!(out, "                    ValidationSeverity::Error,").unwrap();
            writeln!(out, "                    {msg:?}.to_owned(),").unwrap();
            writeln!(out, "                )").unwrap();
            writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
            writeln!(out, "                .with_segment({tag:?}.to_owned())").unwrap();
            writeln!(out, "            );").unwrap();
            writeln!(out, "        }}").unwrap();
        }

        // ── U — AND conjunction: both A and B must appear ────────────────────
        AhbOperator::U => {
            let b_tag = rule
                .secondary_tag
                .as_deref()
                .unwrap_or_else(|| {
                    eprintln!(
                        "  error: AHB PID {pid} segment {tag} cond {idx} operator=U missing secondary_tag"
                    );
                    "MISSING"
                });
            let msg_a = format!(
                "operator U violation for Pruefidentifikator {pid}: \
                 both {when_tag} and {b_tag} are required, but {when_tag} is absent"
            );
            let msg_b = format!(
                "operator U violation for Pruefidentifikator {pid}: \
                 both {when_tag} and {b_tag} are required, but {b_tag} is absent"
            );
            writeln!(out, "        if !({a_present_expr}) {{").unwrap();
            writeln!(out, "            issues.push(").unwrap();
            writeln!(out, "                ValidationIssue::new(ValidationSeverity::Error, {msg_a:?}.to_owned())").unwrap();
            writeln!(out, "                    .with_rule_id({rule_id:?})").unwrap();
            writeln!(
                out,
                "                    .with_segment({when_tag:?}.to_owned())"
            )
            .unwrap();
            writeln!(out, "            );").unwrap();
            writeln!(out, "        }}").unwrap();
            writeln!(
                out,
                "        if !segments.iter().any(|s| s.tag == {b_tag:?}) {{"
            )
            .unwrap();
            writeln!(out, "            issues.push(").unwrap();
            writeln!(out, "                ValidationIssue::new(ValidationSeverity::Error, {msg_b:?}.to_owned())").unwrap();
            writeln!(out, "                    .with_rule_id({rule_id:?})").unwrap();
            writeln!(
                out,
                "                    .with_segment({b_tag:?}.to_owned())"
            )
            .unwrap();
            writeln!(out, "            );").unwrap();
            writeln!(out, "        }}").unwrap();
        }

        // ── O — OR conjunction: at least one of {A, B} must appear ───────────
        AhbOperator::O => {
            let b_tag = rule
                .secondary_tag
                .as_deref()
                .unwrap_or_else(|| {
                    eprintln!(
                        "  error: AHB PID {pid} segment {tag} cond {idx} operator=O missing secondary_tag"
                    );
                    "MISSING"
                });
            let msg = format!(
                "operator O violation for Pruefidentifikator {pid}: \
                 at least one of {{{when_tag}, {b_tag}}} must appear, but neither is present"
            );
            writeln!(out, "        let a_present = {a_present_expr};").unwrap();
            writeln!(
                out,
                "        let b_present = segments.iter().any(|s| s.tag == {b_tag:?});"
            )
            .unwrap();
            writeln!(out, "        if !a_present && !b_present {{").unwrap();
            writeln!(out, "            issues.push(").unwrap();
            writeln!(out, "                ValidationIssue::new(").unwrap();
            writeln!(out, "                    ValidationSeverity::Error,").unwrap();
            writeln!(out, "                    {msg:?}.to_owned(),").unwrap();
            writeln!(out, "                )").unwrap();
            writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
            writeln!(out, "                .with_segment({tag:?}.to_owned())").unwrap();
            writeln!(out, "            );").unwrap();
            writeln!(out, "        }}").unwrap();
        }

        // ── K — at most one of {A, B, ...} may appear ───────────────────────
        AhbOperator::K => {
            let mut all_tags: Vec<&str> = vec![when_tag.as_str()];
            if let Some(ref b) = rule.secondary_tag {
                all_tags.push(b.as_str());
            }
            for t in &rule.additional_tags {
                all_tags.push(t.as_str());
            }
            let tags_display = all_tags.join(", ");
            let msg = format!(
                "operator K violation for Pruefidentifikator {pid}: \
                 at most one of {{{tags_display}}} may appear, but multiple are present"
            );
            for (i, t) in all_tags.iter().enumerate() {
                writeln!(
                    out,
                    "        let __k{i} = segments.iter().any(|s| s.tag == {t:?});"
                )
                .unwrap();
            }
            let sum_expr: String = (0..all_tags.len())
                .map(|i| format!("__k{i} as usize"))
                .collect::<Vec<_>>()
                .join(" + ");
            writeln!(out, "        if {sum_expr} > 1 {{").unwrap();
            writeln!(out, "            issues.push(").unwrap();
            writeln!(out, "                ValidationIssue::new(").unwrap();
            writeln!(out, "                    ValidationSeverity::Error,").unwrap();
            writeln!(out, "                    {msg:?}.to_owned(),").unwrap();
            writeln!(out, "                )").unwrap();
            writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
            writeln!(out, "            );").unwrap();
            writeln!(out, "        }}").unwrap();
        }

        // ── G — count-threshold gated implication ────────────────────────────
        AhbOperator::G => {
            let n = rule.count_threshold;
            let condition_holds_expr = format!(
                "segments.iter().filter(|s| s.tag == {when_tag:?}{primary_check}{additional_checks}).count() > {n}"
            );
            emit_conditional_consequence(
                out,
                tag,
                pid,
                rule,
                &rule_id,
                &condition_holds_expr,
                &condition_desc,
            );
        }

        // ── I / V / E / Z — single-trigger operators ─────────────────────────
        // V = fires when trigger is absent; E = then_requirement is N; Z = qualifier-gated I/E.
        _ => {
            // For E operator, override then_requirement to "N" implicitly
            let effective_n = rule.operator == AhbOperator::E || rule.then_requirement == "N";
            // For V operator, invert trigger
            let condition_holds_expr = if effective_absent {
                format!("!({a_present_expr})")
            } else {
                a_present_expr.clone()
            };
            writeln!(out, "        let condition_holds = {condition_holds_expr};").unwrap();

            if effective_n {
                let msg = format!(
                    "segment {tag} must not appear for Pruefidentifikator {pid} ({condition_desc})"
                );
                writeln!(out, "        if condition_holds {{").unwrap();
                writeln!(
                    out,
                    "            if segments.iter().any(|s| s.tag == {tag:?}) {{"
                )
                .unwrap();
                writeln!(out, "                issues.push(").unwrap();
                writeln!(out, "                    ValidationIssue::new(").unwrap();
                writeln!(out, "                        ValidationSeverity::Error,").unwrap();
                writeln!(out, "                        {msg:?}.to_owned(),").unwrap();
                writeln!(out, "                    )").unwrap();
                writeln!(out, "                    .with_rule_id({rule_id:?})").unwrap();
                writeln!(out, "                    .with_segment({tag:?}.to_owned())").unwrap();
                writeln!(out, "                );").unwrap();
                writeln!(out, "            }}").unwrap();
                writeln!(out, "        }}").unwrap();
            } else {
                // then_requirement is "M" (error) or "S" (Soll = warning)
                let severity = if rule.then_requirement == "S" {
                    "ValidationSeverity::Warning"
                } else {
                    "ValidationSeverity::Error"
                };
                let presence_check_expr = if let Some(ref qval) = rule.then_qualifier_value {
                    let qidx = rule.then_qualifier_index;
                    format!(
                        "!segments.iter().any(|s| s.tag == {tag:?} && s.element_str({qidx}).is_some_and(|v| v == {qval:?}))"
                    )
                } else {
                    format!("!segments.iter().any(|s| s.tag == {tag:?})")
                };
                // M = mandatory (missing = Error); S = Soll (missing = Warning).
                let obligation = if rule.then_requirement == "S" {
                    "should be present"
                } else {
                    "is missing"
                };
                let msg = if let Some(ref qval) = rule.then_qualifier_value {
                    let qidx = rule.then_qualifier_index;
                    format!(
                        "conditional segment {tag} (DE[{qidx}]={qval:?}) {obligation} for Pruefidentifikator {pid} ({condition_desc})"
                    )
                } else {
                    format!(
                        "conditional segment {tag} {obligation} for Pruefidentifikator {pid} ({condition_desc})"
                    )
                };
                writeln!(
                    out,
                    "        if condition_holds && {presence_check_expr} {{"
                )
                .unwrap();
                writeln!(out, "            issues.push(").unwrap();
                writeln!(out, "                ValidationIssue::new(").unwrap();
                writeln!(out, "                    {severity},").unwrap();
                writeln!(out, "                    {msg:?}.to_owned(),").unwrap();
                writeln!(out, "                )").unwrap();
                writeln!(out, "                .with_rule_id({rule_id:?})").unwrap();
                writeln!(out, "                .with_segment({tag:?}.to_owned())").unwrap();
                writeln!(out, "            );").unwrap();
                writeln!(out, "        }}").unwrap();
            }
        }
    }

    writeln!(out, "        for __i in &mut issues[__start..] {{").unwrap();
    writeln!(
        out,
        "            __i.context.push((\"pid\".to_owned(), \"{pid}\".to_owned()));"
    )
    .unwrap();
    writeln!(out, "        }}").unwrap();
    writeln!(out, "    }}").unwrap();
}

/// Emit the consequence block for G-operator rules (condition expression already built).
fn emit_conditional_consequence(
    out: &mut String,
    tag: &str,
    pid: u32,
    rule: &AhbConditionalRule,
    rule_id: &str,
    condition_holds_expr: &str,
    condition_desc: &str,
) {
    if rule.then_requirement == "N" {
        let msg = format!(
            "segment {tag} must not appear for Pruefidentifikator {pid} ({condition_desc})"
        );
        writeln!(out, "        if {condition_holds_expr} {{").unwrap();
        writeln!(
            out,
            "            if segments.iter().any(|s| s.tag == {tag:?}) {{"
        )
        .unwrap();
        writeln!(out, "                issues.push(").unwrap();
        writeln!(out, "                    ValidationIssue::new(ValidationSeverity::Error, {msg:?}.to_owned())").unwrap();
        writeln!(out, "                        .with_rule_id({rule_id:?})").unwrap();
        writeln!(
            out,
            "                        .with_segment({tag:?}.to_owned())"
        )
        .unwrap();
        writeln!(out, "                );").unwrap();
        writeln!(out, "            }}").unwrap();
        writeln!(out, "        }}").unwrap();
    } else {
        // then_requirement "M" = Error, "S" = Soll (Warning) (F-008 fix).
        let severity = if rule.then_requirement == "S" {
            "ValidationSeverity::Warning"
        } else {
            "ValidationSeverity::Error"
        };
        let presence_check_expr = if let Some(ref qval) = rule.then_qualifier_value {
            let qidx = rule.then_qualifier_index;
            format!(
                "!segments.iter().any(|s| s.tag == {tag:?} && s.element_str({qidx}).is_some_and(|v| v == {qval:?}))"
            )
        } else {
            format!("!segments.iter().any(|s| s.tag == {tag:?})")
        };
        // M = mandatory (missing = Error); S = Soll (missing = Warning).
        let obligation = if rule.then_requirement == "S" {
            "should be present"
        } else {
            "is missing"
        };
        let msg = if let Some(ref qval) = rule.then_qualifier_value {
            let qidx = rule.then_qualifier_index;
            format!(
                "conditional segment {tag} (DE[{qidx}]={qval:?}) {obligation} for Pruefidentifikator {pid} ({condition_desc})"
            )
        } else {
            format!(
                "conditional segment {tag} {obligation} for Pruefidentifikator {pid} ({condition_desc})"
            )
        };
        writeln!(
            out,
            "        if {condition_holds_expr} && {presence_check_expr} {{"
        )
        .unwrap();
        writeln!(out, "            issues.push(").unwrap();
        writeln!(
            out,
            "                ValidationIssue::new({severity}, {msg:?}.to_owned())"
        )
        .unwrap();
        writeln!(out, "                    .with_rule_id({rule_id:?})").unwrap();
        writeln!(out, "                    .with_segment({tag:?}.to_owned())").unwrap();
        writeln!(out, "            );").unwrap();
        writeln!(out, "        }}").unwrap();
    }
}

fn emit_profile_impl(out: &mut String, p: &ProfileData, struct_name: &str, _feature: &str) {
    let release_str = &p.release;
    let message_type_variant = {
        let mut s = p.message_type.to_lowercase();
        if let Some(c) = s.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        s
    };
    let lazy_name = format!(
        "RELEASE_{}",
        module_name(&p.message_type, &p.folder_name).to_uppercase()
    );

    // Emit valid_from as a `time::Date` constant expression, or `None`.
    let valid_from_expr = match p.valid_from {
        Some((y, m, d)) => format!("Some(::time::macros::date!({y:04}-{m:02}-{d:02}))"),
        None => "None".to_owned(),
    };

    // Emit valid_until as a `time::Date` constant expression, or `None`.
    let valid_until_expr = match p.valid_until.as_deref() {
        Some(date_str) => {
            // Parse "YYYY-MM-DD" for the macro invocation.
            let parts: Vec<&str> = date_str.split('-').collect();
            if parts.len() == 3 {
                let y = parts[0];
                let m = parts[1];
                let d = parts[2];
                format!("Some(::time::macros::date!({y}-{m}-{d}))")
            } else {
                eprintln!(
                    "  warning: invalid valid_until date '{}' — treating as None",
                    date_str
                );
                "None".to_owned()
            }
        }
        None => "None".to_owned(),
    };

    let ahb_revision_expr = match p.ahb_revision.as_deref() {
        Some(r) => format!("Some({r:?})"),
        None => "None".to_owned(),
    };
    let source_document_expr = match p.source_document.as_deref() {
        Some(s) => format!("Some({s:?})"),
        None => "None".to_owned(),
    };

    writeln!(out).unwrap();
    writeln!(
        out,
        "    static {lazy_name}: LazyLock<Release> = LazyLock::new(|| Release::new({release_str:?}));"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "    pub(crate) struct {struct_name};").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "    impl Profile for {struct_name} {{").unwrap();
    writeln!(
        out,
        "        fn message_type(&self) -> MessageType {{ MessageType::{message_type_variant} }}"
    )
    .unwrap();
    writeln!(
        out,
        "        fn release(&self) -> &Release {{ &{lazy_name} }}"
    )
    .unwrap();
    writeln!(
        out,
        "        fn valid_from(&self) -> Option<::time::Date> {{ {valid_from_expr} }}"
    )
    .unwrap();
    writeln!(
        out,
        "        fn valid_until(&self) -> Option<::time::Date> {{ {valid_until_expr} }}"
    )
    .unwrap();
    writeln!(
        out,
        "        fn ahb_revision(&self) -> Option<&'static str> {{ {ahb_revision_expr} }}"
    )
    .unwrap();
    writeln!(
        out,
        "        fn source_document(&self) -> Option<&'static str> {{ {source_document_expr} }}"
    )
    .unwrap();
    if p.mig.pid_source == PidSourceJson::RffZ13 {
        writeln!(out, "        fn pid_source(&self) -> crate::registry::PidSource {{ crate::registry::PidSource::RffZ13 }}").unwrap();
    }
    writeln!(
        out,
        "        fn mig_rule_pack(&self) -> Arc<ProfileRulePack> {{ mig_rule_pack() }}"
    )
    .unwrap();
    writeln!(out, "        fn ahb_rule_pack(&self, pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {{ ahb_rule_pack(pid) }}").unwrap();
    writeln!(out, "        fn is_code_valid(&self, de_id: &str, code: &str) -> bool {{ is_code_valid(de_id, code) }}").unwrap();
    writeln!(out, "        fn suggest_code(&self, de_id: &str, code: &str) -> Option<&'static str> {{ suggest_code(de_id, code) }}").unwrap();
    writeln!(out, "        fn segment_lookup(&self, tag: &str) -> Option<&'static SegmentDefinition> {{ segment_lookup(tag) }}").unwrap();
    writeln!(out, "        fn code_list(&self, de_id: &str) -> Option<&'static [&'static str]> {{ code_list(de_id) }}").unwrap();
    writeln!(
        out,
        "        fn directory_validator(&self) -> &'static DirectoryValidator {{ directory_validator() }}"
    )
    .unwrap();
    writeln!(
        out,
        "        fn group_schema(&self) -> &'static [GroupDef] {{ GROUP_SCHEMA }}"
    )
    .unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "    pub(crate) static PROFILE: {struct_name} = {struct_name};"
    )
    .unwrap();
}

// ── mod.rs emission ───────────────────────────────────────────────────────────

fn emit_mod_rs(profiles: &[ProfileData]) -> String {
    let mut out = String::new();

    writeln!(
        out,
        "// @generated — do not edit by hand; run `cargo xtask codegen` to regenerate"
    )
    .unwrap();
    writeln!(out, "//").unwrap();
    writeln!(out, "// Generated profiles: {}", profiles.len()).unwrap();
    // Suppress `used_underscore_binding` on `_profiles` parameter in `register_profiles`:
    // the parameter is used under cfg-gated branches but appears unused when no
    // message-type features are enabled.
    writeln!(out, "#![allow(clippy::used_underscore_binding)]").unwrap();
    writeln!(out).unwrap();

    // The AHB helper functions are shared across all profile modules.
    // This is a hand-written file; it is NOT regenerated by `cargo xtask codegen`.
    writeln!(out, "pub(super) mod ahb_helpers;").unwrap();
    writeln!(out).unwrap();

    for p in profiles {
        let module = module_name(&p.message_type, &p.folder_name);
        let feature = feature_name(&p.message_type);
        if p.archived {
            let archive_feature = archive_feature_name(&p.message_type);
            writeln!(
                out,
                "// Archived profile — excluded from the default build."
            )
            .unwrap();
            writeln!(
                out,
                "// Enable the `{archive_feature}` or `archive` Cargo feature to include."
            )
            .unwrap();
            writeln!(
                out,
                "#[cfg(any(feature = \"{archive_feature}\", feature = \"archive\"))]"
            )
            .unwrap();
        } else {
            writeln!(out, "#[cfg(feature = \"{feature}\")]").unwrap();
        }
        writeln!(out, "mod {module};").unwrap();
    }
    writeln!(out).unwrap();

    writeln!(out, "use crate::registry::Profile;").unwrap();
    writeln!(out).unwrap();
    writeln!(out, "/// Register all generated profiles into `profiles`.").unwrap();
    writeln!(
        out,
        "pub(crate) fn register_profiles(_profiles: &mut Vec<&'static dyn Profile>) {{"
    )
    .unwrap();
    for p in profiles {
        let module = module_name(&p.message_type, &p.folder_name);
        let feature = feature_name(&p.message_type);
        if p.archived {
            let archive_feature = archive_feature_name(&p.message_type);
            writeln!(
                out,
                "    #[cfg(any(feature = \"{archive_feature}\", feature = \"archive\"))]"
            )
            .unwrap();
        } else {
            writeln!(out, "    #[cfg(feature = \"{feature}\")]").unwrap();
        }
        writeln!(out, "    _profiles.push(&{module}::PROFILE);").unwrap();
    }
    writeln!(out, "}}").unwrap();

    // ── schema-version integrity check ────────────────────────────────────────
    // Compile-time assertion that every generated module declares a matching
    // CODEGEN_SCHEMA_VERSION; any mismatch is a sign of a partial or stale regen.
    writeln!(out).unwrap();
    writeln!(
        out,
        "/// Compile-time guard: every generated profile module must declare"
    )
    .unwrap();
    writeln!(
        out,
        "/// `CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION`."
    )
    .unwrap();
    writeln!(
        out,
        "/// Regenerate with `cargo xtask codegen` if this fails."
    )
    .unwrap();
    writeln!(out, "#[allow(dead_code)]").unwrap();
    writeln!(
        out,
        "pub(crate) const CURRENT_CODEGEN_SCHEMA_VERSION: u32 = 1;"
    )
    .unwrap();
    for p in profiles {
        let module = module_name(&p.message_type, &p.folder_name);
        let feature = feature_name(&p.message_type);
        if p.archived {
            let archive_feature = archive_feature_name(&p.message_type);
            writeln!(
                out,
                "#[cfg(any(feature = \"{archive_feature}\", feature = \"archive\"))]"
            )
            .unwrap();
        } else {
            writeln!(out, "#[cfg(feature = \"{feature}\")]").unwrap();
        }
        writeln!(
            out,
            "const _: () = assert!({module}::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);"
        )
        .unwrap();
    }

    // ── releases:: module ─────────────────────────────────────────────────────
    writeln!(out).unwrap();
    writeln!(
        out,
        "/// Well-known release identifiers for all registered profiles."
    )
    .unwrap();
    writeln!(out, "///").unwrap();
    writeln!(
        out,
        "/// Use these instead of `Release::new(\"...\")` to get a compile error"
    )
    .unwrap();
    writeln!(
        out,
        "/// when a profile is removed or renamed after a BDEW format update."
    )
    .unwrap();
    writeln!(out, "///").unwrap();
    writeln!(out, "/// # Example").unwrap();
    writeln!(out, "/// ```rust,ignore").unwrap();
    writeln!(out, "/// use edi_energy::releases;").unwrap();
    writeln!(out, "/// use edi_energy::builders::MsconsBuilder;").unwrap();
    writeln!(
        out,
        "/// let msg = MsconsBuilder::new(releases::mscons_fv20261001().clone())"
    )
    .unwrap();
    writeln!(out, "///     .sender(\"9900000000002\")").unwrap();
    writeln!(out, "///     .receiver(\"9900000000003\")").unwrap();
    writeln!(out, "///     .build();").unwrap();
    writeln!(out, "/// ```").unwrap();
    writeln!(out, "pub mod releases {{").unwrap();
    // The `use` statements are only needed when at least one message feature is
    // enabled; gate them to avoid unused-import warnings with --no-default-features.
    // Include both normal features and archive features in the `any()` guard.
    let all_features: String = profiles
        .iter()
        .flat_map(|p| {
            let f = feature_name(&p.message_type);
            let af = archive_feature_name(&p.message_type);
            [
                format!("feature = \"{f}\""),
                format!("feature = \"{af}\""),
                "feature = \"archive\"".to_owned(),
            ]
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "    #[cfg(any({all_features}))]").unwrap();
    writeln!(out, "    use std::sync::LazyLock;").unwrap();
    writeln!(out, "    #[cfg(any({all_features}))]").unwrap();
    writeln!(out, "    use crate::Release;").unwrap();
    for p in profiles {
        let fn_name = module_name(&p.message_type, &p.folder_name);
        let feature = feature_name(&p.message_type);
        let release_str = &p.release;
        writeln!(out).unwrap();
        writeln!(
            out,
            "    /// Release `{release_str}` — valid from profile directory `{}`.",
            p.folder_name
        )
        .unwrap();
        if p.archived {
            let archive_feature = archive_feature_name(&p.message_type);
            writeln!(out, "    /// This profile is archived. Enable `{archive_feature}` or `archive` to use it.").unwrap();
            writeln!(
                out,
                "    #[cfg(any(feature = \"{archive_feature}\", feature = \"archive\"))]"
            )
            .unwrap();
        } else {
            writeln!(out, "    #[cfg(feature = \"{feature}\")]").unwrap();
        }
        writeln!(out, "    pub fn {fn_name}() -> &'static Release {{").unwrap();
        writeln!(
            out,
            "        static R: LazyLock<Release> = LazyLock::new(|| Release::new({release_str:?}));"
        )
        .unwrap();
        writeln!(out, "        &R").unwrap();
        writeln!(out, "    }}").unwrap();
    }
    writeln!(out, "}}").unwrap();

    out
}

// ── File writing & formatting ─────────────────────────────────────────────────

/// Write `content` to `path` only if it differs from the existing file contents.
/// Returns `true` if the file was written (new or changed), `false` if unchanged.
fn write_file_if_changed(path: &Path, content: &str) -> bool {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    if existing == content {
        return false;
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("error: cannot create dir {}: {}", parent.display(), e);
            std::process::exit(1);
        });
    }
    std::fs::write(path, content).unwrap_or_else(|e| {
        eprintln!("error: cannot write {}: {}", path.display(), e);
        std::process::exit(1);
    });
    true
}

fn format_generated(dir: &Path) {
    let Some(rustfmt) = which_rustfmt() else {
        // rustfmt not available — generated files will be unformatted.
        // This is acceptable for normal `codegen` runs (non-check mode).
        eprintln!("warning: rustfmt not found — skipping formatting of generated files.");
        return;
    };
    let files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(std::result::Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "rs"))
                .collect()
        })
        .unwrap_or_default();

    if files.is_empty() {
        return;
    }

    let status = std::process::Command::new(&rustfmt)
        .args(&files)
        .status()
        .unwrap_or_else(|e| {
            eprintln!("warning: rustfmt failed: {e}");
            std::process::exit(0);
        });

    if !status.success() {
        eprintln!("warning: rustfmt exited with status {status}");
    }
}

fn which_rustfmt() -> Option<String> {
    // Prefer an explicit override (e.g. set by dtolnay/rust-toolchain action), then
    // the toolchain-local binary, then a bare PATH lookup.
    Some(std::env::var("RUSTFMT").unwrap_or_else(|_| "rustfmt".to_owned())).filter(|bin| {
        // Verify the binary actually exists before returning it, so callers
        // can distinguish "not installed" from "installed but broken".
        std::process::Command::new(bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Format a Rust source string through rustfmt and return the result.
///
/// # Errors
///
/// Returns `Err` when `rustfmt` is not found or exits with a non-zero status.
/// The error message is suitable for display to the user.
fn rustfmt_string(rustfmt_bin: &str, src: String) -> Result<String, String> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    let mut child = Command::new(rustfmt_bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("rustfmt: could not spawn `{rustfmt_bin}`: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(src.as_bytes())
            .map_err(|e| format!("rustfmt: failed to write stdin: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("rustfmt: wait_with_output failed: {e}"))?;
    if output.status.success() {
        String::from_utf8(output.stdout)
            .map_err(|e| format!("rustfmt: output is not valid UTF-8: {e}"))
    } else {
        Err(format!("rustfmt exited with status {}", output.status))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_fv_date ─────────────────────────────────────────────────────────

    #[test]
    fn parse_fv_date_standard() {
        assert_eq!(parse_fv_date("fv20241001"), Some((2024, 10, 1)));
        assert_eq!(parse_fv_date("fv20251001"), Some((2025, 10, 1)));
        assert_eq!(parse_fv_date("fv20260101"), Some((2026, 1, 1)));
        assert_eq!(parse_fv_date("fv20260401"), Some((2026, 4, 1)));
    }

    #[test]
    fn parse_fv_date_with_gas_suffix() {
        assert_eq!(parse_fv_date("fv20241001_gas"), Some((2024, 10, 1)));
        assert_eq!(parse_fv_date("fv20261001_gas"), Some((2026, 10, 1)));
    }

    #[test]
    fn parse_fv_date_legacy_formats() {
        assert_eq!(parse_fv_date("2.4c"), None);
        assert_eq!(parse_fv_date("5.5.3a"), None);
        assert_eq!(parse_fv_date("S2.1"), None);
        assert_eq!(parse_fv_date("G1.1"), None);
        assert_eq!(parse_fv_date("1.1a"), None);
    }

    #[test]
    fn parse_fv_date_invalid() {
        assert_eq!(parse_fv_date(""), None);
        assert_eq!(parse_fv_date("fv"), None);
        assert_eq!(parse_fv_date("fv2024"), None); // too short
        assert_eq!(parse_fv_date("fv202410"), None); // too short
        assert_eq!(parse_fv_date("fvabcdefgh"), None); // non-digits
        assert_eq!(parse_fv_date("fv20241301"), None); // month 13
        assert_eq!(parse_fv_date("fv20241000"), None); // day 0
        assert_eq!(parse_fv_date("fv20240001"), None); // month 0
    }

    // ── module_name ──────────────────────────────────────────────────────────

    #[test]
    fn module_name_standard() {
        assert_eq!(module_name("UTILMD", "fv20241001"), "utilmd_fv20241001");
        assert_eq!(module_name("MSCONS", "fv20251001"), "mscons_fv20251001");
        assert_eq!(module_name("CONTRL", "fv20260101"), "contrl_fv20260101");
    }

    #[test]
    fn module_name_with_suffix() {
        assert_eq!(
            module_name("UTILMD", "fv20241001_gas"),
            "utilmd_fv20241001_gas"
        );
    }

    #[test]
    fn module_name_legacy() {
        assert_eq!(module_name("UTILMD", "5.5.3a"), "utilmd_5_5_3a");
        assert_eq!(module_name("MSCONS", "2.4c"), "mscons_2_4c");
    }

    // ── struct_name ──────────────────────────────────────────────────────────

    #[test]
    fn struct_name_fv_dated() {
        assert_eq!(
            struct_name("UTILMD", "fv20241001"),
            "UtilmdFv20241001Profile"
        );
        assert_eq!(
            struct_name("MSCONS", "fv20251001"),
            "MsconsFv20251001Profile"
        );
        assert_eq!(
            struct_name("CONTRL", "fv20260101"),
            "ContrlFv20260101Profile"
        );
    }

    #[test]
    fn struct_name_with_suffix_is_unique() {
        // Two directories that share a wire code must produce distinct struct names.
        let s1 = struct_name("UTILMD", "fv20241001");
        let s2 = struct_name("UTILMD", "fv20261001");
        assert_ne!(s1, s2);
        assert_eq!(s1, "UtilmdFv20241001Profile");
        assert_eq!(s2, "UtilmdFv20261001Profile");

        // Gas suffix variant also distinct.
        let gas = struct_name("UTILMD", "fv20241001_gas");
        assert_ne!(s1, gas);
        assert_eq!(gas, "UtilmdFv20241001GasProfile");
    }

    #[test]
    fn struct_name_legacy() {
        // Legacy folder names (same as wire codes) produce stable names.
        let n = struct_name("UTILMD", "5.5.3a");
        assert!(n.starts_with("Utilmd"), "expected Utilmd prefix, got {n}");
        assert!(n.ends_with("Profile"), "expected Profile suffix, got {n}");
    }

    // ── mig_segment_sequence ─────────────────────────────────────────────────

    #[test]
    fn mig_segment_sequence_uses_ordering_hint() {
        let mig = MigProfile {
            schema_version: 1,
            message_type: "MSCONS".into(),
            release: "2.4c".into(),
            segments: vec![],
            segment_groups: vec![],
            ordering_hint: vec!["UNH".into(), "BGM".into(), "UNS".into(), "UNT".into()],
            pid_source: PidSourceJson::BgmDe1004,
            valid_from: None,
            valid_until: None,
            ahb_revision: None,
            source_document: None,
            supersedes_directory: None,
            archived: false,
            pid_exempt: false,
        };
        let seq = mig_segment_sequence(&mig);
        assert_eq!(seq, vec!["UNH", "BGM", "UNS", "UNT"]);
    }

    #[test]
    fn mig_segment_sequence_auto_derives_with_unt() {
        let mig = MigProfile {
            schema_version: 1,
            message_type: "APERAK".into(),
            release: "2.1i".into(),
            segments: vec![
                MigSegment {
                    tag: "UNH".into(),
                    name: "".into(),
                    mandatory: true,
                    max_occurrences: 1,
                    elements: vec![],
                    qualifier_restrictions: Default::default(),
                },
                MigSegment {
                    tag: "BGM".into(),
                    name: "".into(),
                    mandatory: true,
                    max_occurrences: 1,
                    elements: vec![],
                    qualifier_restrictions: Default::default(),
                },
                MigSegment {
                    tag: "UNT".into(),
                    name: "".into(),
                    mandatory: true,
                    max_occurrences: 1,
                    elements: vec![],
                    qualifier_restrictions: Default::default(),
                },
            ],
            segment_groups: vec![],
            ordering_hint: vec![],
            pid_source: PidSourceJson::BgmDe1004,
            valid_from: None,
            valid_until: None,
            ahb_revision: None,
            source_document: None,
            supersedes_directory: None,
            archived: false,
            pid_exempt: false,
        };
        let seq = mig_segment_sequence(&mig);
        // UNT must be last
        assert_eq!(seq.last().map(std::string::String::as_str), Some("UNT"));
        assert_eq!(seq[0], "UNH");
        assert_eq!(seq[1], "BGM");
    }

    // ── mig_segment_sequence: group trigger inclusion (F-001 regression guard) ─

    /// Verifies that group-trigger segments not in `mig.segments` are included in
    /// the derived EXPECTED_ORDER — the critical F-001 fix.  A UTILMD-shaped profile
    /// (UNH, BGM, DTM, UNT at top level; RFF/NAD/IDE as group triggers) must produce
    /// a sequence that includes the group-trigger tags.
    #[test]
    fn mig_segment_sequence_includes_group_triggers_not_in_top_level() {
        fn seg(tag: &str) -> MigSegment {
            MigSegment {
                tag: tag.into(),
                name: "".into(),
                mandatory: true,
                max_occurrences: 1,
                elements: vec![],
                qualifier_restrictions: Default::default(),
            }
        }
        fn grp(id: &str, trigger: &str, segs: Vec<MigSegment>) -> MigGroup {
            MigGroup {
                id: id.into(),
                trigger_segment: trigger.into(),
                mandatory: true,
                max_occurrences: 1,
                min_occurrences: None,
                segments: segs,
                groups: vec![],
            }
        }
        let mig = MigProfile {
            schema_version: 1,
            message_type: "UTILMD".into(),
            release: "S2.2".into(),
            // Top-level segments: UNH, BGM, DTM, UNT — no group triggers here.
            segments: vec![seg("UNH"), seg("BGM"), seg("DTM"), seg("UNT")],
            // First-level groups: SG1 (RFF), SG2 (NAD), SG4 (IDE).
            segment_groups: vec![
                grp("SG1", "RFF", vec![seg("RFF")]),
                grp("SG2", "NAD", vec![seg("NAD")]),
                grp("SG4", "IDE", vec![seg("IDE")]),
            ],
            ordering_hint: vec![],
            pid_source: PidSourceJson::BgmDe1004,
            valid_from: None,
            valid_until: None,
            ahb_revision: None,
            source_document: None,
            supersedes_directory: None,
            archived: false,
            pid_exempt: false,
        };
        let seq = mig_segment_sequence(&mig);
        // UNH must be first, UNT must be last.
        assert_eq!(seq.first().map(std::string::String::as_str), Some("UNH"));
        assert_eq!(seq.last().map(std::string::String::as_str), Some("UNT"));
        // Group triggers must appear between UNH and UNT.
        assert!(
            seq.contains(&"RFF".to_owned()),
            "RFF (SG1 trigger) must be in EXPECTED_ORDER"
        );
        assert!(
            seq.contains(&"NAD".to_owned()),
            "NAD (SG2 trigger) must be in EXPECTED_ORDER"
        );
        assert!(
            seq.contains(&"IDE".to_owned()),
            "IDE (SG4 trigger) must be in EXPECTED_ORDER"
        );
        // DTM appears in top-level; must NOT be duplicated.
        assert_eq!(
            seq.iter().filter(|t| t.as_str() == "DTM").count(),
            1,
            "DTM must appear exactly once"
        );
    }

    /// Verifies that a group trigger that ALSO appears in `mig.segments` is NOT
    /// duplicated in the derived order (prevents false ordering violations for
    /// segments used both at the top level and as group triggers).
    #[test]
    fn mig_segment_sequence_no_duplicates_for_shared_triggers() {
        fn seg(tag: &str) -> MigSegment {
            MigSegment {
                tag: tag.into(),
                name: "".into(),
                mandatory: true,
                max_occurrences: 1,
                elements: vec![],
                qualifier_restrictions: Default::default(),
            }
        }
        fn grp(id: &str, trigger: &str, segs: Vec<MigSegment>) -> MigGroup {
            MigGroup {
                id: id.into(),
                trigger_segment: trigger.into(),
                mandatory: true,
                max_occurrences: 1,
                min_occurrences: None,
                segments: segs,
                groups: vec![],
            }
        }
        let mig = MigProfile {
            schema_version: 1,
            message_type: "UTILMD".into(),
            release: "S2.2".into(),
            // DTM appears both in top-level segments AND as a group trigger in SG0.
            segments: vec![seg("UNH"), seg("BGM"), seg("DTM"), seg("UNT")],
            segment_groups: vec![grp("SG0", "DTM", vec![seg("DTM")])],
            ordering_hint: vec![],
            pid_source: PidSourceJson::BgmDe1004,
            valid_from: None,
            valid_until: None,
            ahb_revision: None,
            source_document: None,
            supersedes_directory: None,
            archived: false,
            pid_exempt: false,
        };
        let seq = mig_segment_sequence(&mig);
        assert_eq!(
            seq.iter().filter(|t| t.as_str() == "DTM").count(),
            1,
            "DTM used as both top-level seg and group trigger must appear exactly once"
        );
    }

    // ── parse_fv_date boundary month/day checks ───────────────────────────────

    #[test]
    fn parse_fv_date_boundary_months() {
        assert_eq!(parse_fv_date("fv20260131"), Some((2026, 1, 31)));
        assert_eq!(parse_fv_date("fv20261231"), Some((2026, 12, 31)));
    }

    // ── date arithmetic (F-010 prune-expired) ─────────────────────────────────

    #[test]
    fn date_to_unix_days_unix_epoch() {
        // 1970-01-01 is day 0
        assert_eq!(date_to_unix_days(1970, 1, 1), 0);
    }

    #[test]
    fn date_to_unix_days_known_dates() {
        // 2025-09-30: mscons/fv20240401 valid_until
        assert_eq!(date_to_unix_days(2025, 9, 30), 20361);
        // 2026-06-11: approx "today" in tests
        assert_eq!(date_to_unix_days(2026, 6, 11), 20615);
        // Leap day
        assert_eq!(date_to_unix_days(2024, 2, 29), 19782);
    }

    #[test]
    fn add_days_round_trip() {
        let start = (2025, 9, 30u8);
        assert_eq!(add_days(start, 0), start);
        // +1 day across month boundary
        assert_eq!(add_days(start, 1), (2025, 10, 1));
        // +90 days → 2025-12-29
        assert_eq!(add_days(start, 90), (2025, 12, 29));
        // +366 days (leap 2026 is not leap, 2025 is not leap) → +366 from 2025-09-30 = 2026-10-01
        assert_eq!(add_days(start, 366), (2026, 10, 1));
    }

    #[test]
    fn add_days_across_feb_leap() {
        // 2024-02-28 + 1 = 2024-02-29 (2024 is a leap year)
        assert_eq!(add_days((2024, 2, 28), 1), (2024, 2, 29));
        // 2024-02-29 + 1 = 2024-03-01
        assert_eq!(add_days((2024, 2, 29), 1), (2024, 3, 1));
        // 2023-02-28 + 1 = 2023-03-01 (2023 is not a leap year)
        assert_eq!(add_days((2023, 2, 28), 1), (2023, 3, 1));
    }

    #[test]
    fn parse_date_str_valid() {
        assert_eq!(parse_date_str("2025-09-30"), Some((2025, 9, 30)));
        assert_eq!(parse_date_str("2026-06-11"), Some((2026, 6, 11)));
    }

    #[test]
    fn parse_date_str_invalid() {
        assert_eq!(parse_date_str("not-a-date"), None);
        assert_eq!(parse_date_str("2025-13-01"), None); // month > 12
        assert_eq!(parse_date_str("2025-00-01"), None); // month == 0
        assert_eq!(parse_date_str("2025-06"), None); // too short
    }

    #[test]
    fn date_before_ordering() {
        assert!(date_before((2025, 9, 30), (2026, 6, 11)));
        assert!(date_before((2025, 12, 31), (2026, 1, 1)));
        assert!(!date_before((2026, 1, 1), (2025, 12, 31)));
        assert!(!date_before((2025, 9, 30), (2025, 9, 30))); // equal is not strictly before
    }

    #[test]
    fn archive_feature_name_format() {
        assert_eq!(archive_feature_name("MSCONS"), "mscons-archive");
        assert_eq!(archive_feature_name("UTILMD"), "utilmd-archive");
        assert_eq!(archive_feature_name("contrl"), "contrl-archive");
    }

    // ── ahb_conditional_rule_fn_name ──────────────────────────────────────────

    #[test]
    fn conditional_fn_name_format() {
        assert_eq!(
            ahb_conditional_rule_fn_name(13017, "STS", 0),
            "rule_ahb_13017_sts_cond_0"
        );
        assert_eq!(
            ahb_conditional_rule_fn_name(13017, "STS", 3),
            "rule_ahb_13017_sts_cond_3"
        );
        assert_eq!(
            ahb_conditional_rule_fn_name(55001, "DTM", 0),
            "rule_ahb_55001_dtm_cond_0"
        );
    }

    // ── emit_ahb_pid_rule_fn: not-used inline closure ─────────────────────────

    #[test]
    fn emit_not_used_rule_inline_closure() {
        // Requirement=N with no conditional_rules → inline closure calling ahb_check_not_used.
        let pid = PruefidentifikatorEntry {
            code: 13002,
            name: "Not-used test".into(),
            segment_rules: vec![AhbSegmentRule {
                tag: "STS".into(),
                requirement: "N".into(),
                qualifier_restrictions: std::collections::BTreeMap::new(),
                field_rules: vec![],
                required_qualifiers: std::collections::BTreeMap::new(),
                conditional_rules: vec![],
                description: String::new(),
            }],
            group_rules: vec![],
        };
        let mut out = String::new();
        emit_ahb_pid_rule_fn(&mut out, &pid, "MSCONS", "fv20261001");
        // Must use the shared helper
        assert!(
            out.contains("ahb_check_not_used"),
            "should call ahb_check_not_used: {out}"
        );
        assert!(out.contains("\"STS\""), "tag missing: {out}");
        assert!(out.contains("AHB-13002-STS-N"), "rule_id missing: {out}");
        assert!(out.contains("must not appear"), "message missing: {out}");
        // Must NOT emit a standalone named function
        assert!(
            !out.contains("fn rule_ahb_13002_sts_not_used("),
            "should not emit standalone fn: {out}"
        );
    }

    // ── emit_ahb_conditional_rule_fn: if_present + M ──────────────────────────

    #[test]
    fn emit_conditional_rule_if_present_mandatory() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::I,
            when_tag: "QTY".into(),
            secondary_tag: None,
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: Some("67".into()),
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: "M".into(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "[92] Wenn QTY DE6063 mit Wert 67 vorhanden".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13017, "STS", 0, &rule);

        assert!(
            out.contains("fn rule_ahb_13017_sts_cond_0("),
            "fn signature: {out}"
        );
        assert!(out.contains("AHB-13017-STS-I0"), "rule_id: {out}");
        // Condition check: trigger present
        assert!(out.contains("s.tag == \"QTY\""), "trigger tag: {out}");
        assert!(out.contains("v == \"67\""), "trigger value: {out}");
        assert!(
            !out.contains("!condition_holds"),
            "must not negate condition: {out}"
        );
        // Consequence: segment must be present (if NOT found → error)
        assert!(
            out.contains("!segments.iter().any"),
            "must check absence of target: {out}"
        );
        assert!(
            out.contains("conditional segment STS is missing"),
            "message: {out}"
        );
    }

    #[test]
    fn emit_conditional_rule_if_present_mandatory_with_qualifier() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::I,
            when_tag: "QTY".into(),
            secondary_tag: None,
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: Some("67".into()),
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: "M".into(),
            then_qualifier_index: 0,
            then_qualifier_value: Some("Z32".into()),
            description: "[92] wenn QTY=67 vorhanden → STS mit Z32".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13017, "STS", 0, &rule);

        // Qualifier check: must check element value Z32 in target segment
        assert!(
            out.contains("v == \"Z32\""),
            "qualifier value check missing: {out}"
        );
        // Presence check uses qualifier-specific form (not bare tag check)
        assert!(
            !out.contains("!segments.iter().any(|s| s.tag == \"STS\")"),
            "must use qualifier form, not bare tag: {out}"
        );
        // Error message includes qualifier info (appears as escaped literal in generated source)
        assert!(out.contains("DE[0]="), "qualifier index in message: {out}");
        assert!(
            out.contains(r#"\"Z32\""#),
            "qualifier value in message: {out}"
        );
    }

    // ── emit_ahb_conditional_rule_fn: if_absent + M ───────────────────────────

    #[test]
    fn emit_conditional_rule_if_absent_mandatory() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::V,
            when_tag: "RFF".into(),
            secondary_tag: None,
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: Some("AGK".into()),
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: "M".into(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "[131] wenn RFF+AGK nicht vorhanden".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13017, "LOC", 1, &rule);

        assert!(
            out.contains("fn rule_ahb_13017_loc_cond_1("),
            "fn signature: {out}"
        );
        // Condition is negated: fires when RFF+AGK is ABSENT
        assert!(out.contains("!("), "negation: {out}");
        assert!(out.contains("s.tag == \"RFF\""), "trigger tag: {out}");
        assert!(out.contains("v == \"AGK\""), "trigger value: {out}");
        assert!(
            out.contains("conditional segment LOC is missing"),
            "message: {out}"
        );
    }

    // ── emit_ahb_conditional_rule_fn: if_present + N ──────────────────────────

    #[test]
    fn emit_conditional_rule_if_present_not_used() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::E,
            when_tag: "STS".into(),
            secondary_tag: None,
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: None,
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: "N".into(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "when STS is present, FOO must not appear".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13002, "FOO", 0, &rule);

        assert!(
            out.contains("fn rule_ahb_13002_foo_cond_0("),
            "fn signature: {out}"
        );
        // Condition: segment present (no value check)
        assert!(out.contains("s.tag == \"STS\""), "trigger tag: {out}");
        assert!(
            !out.contains("when_value"),
            "no value check should be emitted: {out}"
        );
        // Consequence: must NOT appear (if ANY found → error)
        assert!(
            out.contains("segments.iter().any(|s| s.tag == \"FOO\")"),
            "presence check: {out}"
        );
        assert!(out.contains("must not appear"), "not-used message: {out}");
    }

    // ── conditional_rule_fn_name_uniqueness ───────────────────────────────────

    #[test]
    fn conditional_fn_names_are_unique_per_index() {
        let n0 = ahb_conditional_rule_fn_name(13017, "STS", 0);
        let n1 = ahb_conditional_rule_fn_name(13017, "STS", 1);
        let n2 = ahb_conditional_rule_fn_name(13017, "STS", 2);
        assert_ne!(n0, n1);
        assert_ne!(n1, n2);
    }

    // ── emit_ahb_conditional_rule_fn: operator X (XOR) ───────────────────────

    #[test]
    fn emit_conditional_rule_xor() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::X,
            when_tag: "STS".into(),
            secondary_tag: Some("CTA".into()),
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: None,
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: String::new(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "exactly one of STS or CTA must appear".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13017, "STS", 0, &rule);

        assert!(
            out.contains("fn rule_ahb_13017_sts_cond_0("),
            "fn signature: {out}"
        );
        assert!(out.contains("AHB-13017-STS-X0"), "rule_id: {out}");
        // XOR: both or neither is an error
        assert!(out.contains("a_present == b_present"), "XOR check: {out}");
        assert!(out.contains("s.tag == \"STS\""), "tag A: {out}");
        assert!(out.contains("s.tag == \"CTA\""), "tag B: {out}");
        assert!(out.contains("exactly one"), "XOR message: {out}");
    }

    // ── emit_ahb_conditional_rule_fn: operator U (AND) ───────────────────────

    #[test]
    fn emit_conditional_rule_and() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::U,
            when_tag: "STS".into(),
            secondary_tag: Some("NAD".into()),
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: None,
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: String::new(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "both STS and NAD must appear".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13017, "STS", 1, &rule);

        assert!(
            out.contains("fn rule_ahb_13017_sts_cond_1("),
            "fn signature: {out}"
        );
        assert!(out.contains("AHB-13017-STS-U1"), "rule_id: {out}");
        // U: each absent is a separate error
        assert!(out.contains("s.tag == \"STS\""), "tag A: {out}");
        assert!(out.contains("s.tag == \"NAD\""), "tag B: {out}");
        assert!(
            out.contains("both STS and NAD are required"),
            "AND message A: {out}"
        );
    }

    // ── emit_ahb_conditional_rule_fn: operator O (OR) ────────────────────────

    #[test]
    fn emit_conditional_rule_or() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::O,
            when_tag: "STS".into(),
            secondary_tag: Some("LOC".into()),
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: None,
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: String::new(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "at least one of STS or LOC must appear".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13002, "STS", 0, &rule);

        assert!(
            out.contains("fn rule_ahb_13002_sts_cond_0("),
            "fn signature: {out}"
        );
        assert!(out.contains("AHB-13002-STS-O0"), "rule_id: {out}");
        // O: fires when neither is present
        assert!(out.contains("!a_present && !b_present"), "OR check: {out}");
        assert!(out.contains("s.tag == \"STS\""), "tag A: {out}");
        assert!(out.contains("s.tag == \"LOC\""), "tag B: {out}");
        assert!(out.contains("at least one of"), "OR message: {out}");
    }

    // ── emit_ahb_conditional_rule_fn: operator G (count threshold) ───────────

    #[test]
    fn emit_conditional_rule_count_threshold() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::G,
            when_tag: "QTY".into(),
            secondary_tag: None,
            additional_tags: vec![],
            count_threshold: 2,
            when_element_index: 0,
            when_value: None,
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: "M".into(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "if QTY appears more than 2 times, DTM is required".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13002, "DTM", 0, &rule);

        assert!(
            out.contains("fn rule_ahb_13002_dtm_cond_0("),
            "fn signature: {out}"
        );
        assert!(out.contains("AHB-13002-DTM-G0"), "rule_id: {out}");
        // G: count-based trigger
        assert!(out.contains(".count() > 2"), "count threshold: {out}");
        assert!(out.contains("s.tag == \"QTY\""), "trigger tag: {out}");
        assert!(
            out.contains("conditional segment DTM is missing"),
            "consequence message: {out}"
        );
    }

    // ── emit_ahb_conditional_rule_fn: operator K (at most one) ──────────────

    #[test]
    fn emit_conditional_rule_at_most_one() {
        let rule = AhbConditionalRule {
            operator: AhbOperator::K,
            when_tag: "STS".into(),
            secondary_tag: Some("CTA".into()),
            additional_tags: vec!["RFF".into()],
            count_threshold: 0,
            when_element_index: 0,
            when_value: None,
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: String::new(),
            then_qualifier_index: 0,
            then_qualifier_value: None,
            description: "at most one of STS, CTA, or RFF may appear".into(),
        };
        let mut out = String::new();
        emit_ahb_conditional_rule_fn(&mut out, 13999, "STS", 0, &rule);

        assert!(
            out.contains("fn rule_ahb_13999_sts_cond_0("),
            "fn signature: {out}"
        );
        assert!(out.contains("AHB-13999-STS-K0"), "rule_id: {out}");
        // K: presence checks for all three tags
        assert!(out.contains("s.tag == \"STS\""), "tag A: {out}");
        assert!(out.contains("s.tag == \"CTA\""), "tag B: {out}");
        assert!(out.contains("s.tag == \"RFF\""), "tag C: {out}");
        // K: sum > 1 check
        assert!(out.contains("> 1"), "K count check: {out}");
        // K: error message
        assert!(out.contains("at most one"), "K error message: {out}");
        assert!(
            out.contains("STS, CTA, RFF"),
            "K tag list in message: {out}"
        );
    }

    // ── F-002: "O" (optional/Kann) requirement — inline closure approach ──────

    /// A segment with requirement="O" and a qualifier restriction must emit
    /// a qualifier inline closure (that fires only when the segment is present)
    /// but must NOT emit a mandatory inline closure.
    #[test]
    fn optional_segment_emits_qualifier_inline_but_not_mandatory() {
        let pid_entry = PruefidentifikatorEntry {
            code: 55001,
            name: "Optional LOC test".into(),
            segment_rules: vec![AhbSegmentRule {
                tag: "LOC".into(),
                requirement: "O".into(),
                qualifier_restrictions: {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert("3227".into(), vec!["Z18".into(), "Z19".into()]);
                    m
                },
                field_rules: vec![],
                required_qualifiers: std::collections::BTreeMap::new(),
                conditional_rules: vec![],
                description: String::new(),
            }],
            group_rules: vec![],
        };

        let mut out = String::new();
        emit_ahb_pid_rule_fn(&mut out, &pid_entry, "UTILMD", "fv20241001");

        // Must contain the qualifier rule (inline closure calling ahb_check_qualifier)
        assert!(
            out.contains("ahb_check_qualifier"),
            "qualifier inline missing: {out}"
        );
        assert!(
            out.contains("AHB-55001-LOC-3227-Q"),
            "qualifier rule_id: {out}"
        );
        assert!(out.contains("\"Z18\" | \"Z19\""), "allowed set: {out}");

        // Must NOT contain a mandatory inline closure for LOC
        assert!(
            !out.contains("ahb_check_mandatory"),
            "must not emit mandatory inline for O req: {out}"
        );
        assert!(
            !out.contains("AHB-55001-LOC-M"),
            "must not have mandatory rule_id: {out}"
        );
    }

    // ── F-001: emit_ahb_group_rule ────────────────────────────────────────────

    /// Group rule with requirement=M emits `require_segment_in_group`.
    #[test]
    fn group_rule_mandatory_emits_require_segment_in_group() {
        let gr = AhbGroupRule {
            group_id: "SG4".into(),
            tag: "STS".into(),
            requirement: "M".into(),
            qualifier_restrictions: std::collections::BTreeMap::new(),
            conditional_rules: vec![],
            description: String::new(),
        };
        let mut out = String::new();
        emit_ahb_group_rule(&mut out, 55001, &gr);
        assert!(
            out.contains("require_segment_in_group"),
            "must use require_segment_in_group: {out}"
        );
        assert!(out.contains("\"SG4\""), "group scope: {out}");
        assert!(out.contains("\"STS\""), "tag: {out}");
        assert!(out.contains("AHB-55001-SG4-STS-M"), "rule_id: {out}");
    }

    /// Group rule with requirement=N emits `forbid_segment_in_group`.
    #[test]
    fn group_rule_not_used_emits_forbid_segment_in_group() {
        let gr = AhbGroupRule {
            group_id: "SG3".into(),
            tag: "FTX".into(),
            requirement: "N".into(),
            qualifier_restrictions: std::collections::BTreeMap::new(),
            conditional_rules: vec![],
            description: String::new(),
        };
        let mut out = String::new();
        emit_ahb_group_rule(&mut out, 29001, &gr);
        assert!(
            out.contains("forbid_segment_in_group"),
            "must use forbid_segment_in_group: {out}"
        );
        assert!(out.contains("\"SG3\""), "group scope: {out}");
        assert!(out.contains("\"FTX\""), "tag: {out}");
        assert!(out.contains("AHB-29001-SG3-FTX-N"), "rule_id: {out}");
    }

    /// Group rule with qualifier_restrictions emits `with_scoped_group_rule_fn`
    /// calling `ahb_check_qualifier`.
    #[test]
    fn group_rule_qualifier_restriction_emits_scoped_closure() {
        let mut qr = std::collections::BTreeMap::new();
        qr.insert("3035".into(), vec!["MS".into(), "MR".into()]);
        let gr = AhbGroupRule {
            group_id: "SG3".into(),
            tag: "NAD".into(),
            requirement: "M".into(),
            qualifier_restrictions: qr,
            conditional_rules: vec![],
            description: String::new(),
        };
        let mut out = String::new();
        emit_ahb_group_rule(&mut out, 29001, &gr);
        assert!(
            out.contains("with_scoped_group_rule_fn"),
            "must use with_scoped_group_rule_fn: {out}"
        );
        assert!(
            out.contains("ahb_check_qualifier"),
            "must call ahb_check_qualifier: {out}"
        );
        assert!(out.contains("AHB-29001-SG3-NAD-3035-Q"), "rule_id: {out}");
        assert!(out.contains("\"MS\" | \"MR\""), "allowed set: {out}");
    }

    /// Group rule with requirement=O and qualifier_restrictions emits qualifier
    /// closure but no require/forbid call.
    #[test]
    fn group_rule_optional_with_qualifier_emits_only_qualifier_closure() {
        let mut qr = std::collections::BTreeMap::new();
        qr.insert("1001".into(), vec!["E01".into(), "E03".into()]);
        let gr = AhbGroupRule {
            group_id: "SG4".into(),
            tag: "BGM".into(),
            requirement: "O".into(),
            qualifier_restrictions: qr,
            conditional_rules: vec![],
            description: String::new(),
        };
        let mut out = String::new();
        emit_ahb_group_rule(&mut out, 55001, &gr);
        assert!(
            out.contains("with_scoped_group_rule_fn"),
            "must have scoped closure: {out}"
        );
        assert!(
            !out.contains("require_segment_in_group"),
            "O should not emit require: {out}"
        );
        assert!(
            !out.contains("forbid_segment_in_group"),
            "O should not emit forbid: {out}"
        );
    }

    /// Group rule with conditional_rules emits `with_scoped_group_rule_fn` closures
    /// containing the per-instance conditional logic (F-001 fix).
    #[test]
    fn group_rule_conditional_emits_group_conditional_closure() {
        let cond = AhbConditionalRule {
            operator: AhbOperator::I,
            when_tag: "QTY".into(),
            secondary_tag: None,
            additional_tags: vec![],
            count_threshold: 0,
            when_element_index: 0,
            when_value: Some("67".into()),
            when_value_alternatives: vec![],
            when_additional_elements: vec![],
            then_requirement: "M".into(),
            then_qualifier_index: 0,
            then_qualifier_value: Some("Z32".into()),
            description:
                "[92] Wenn QTY DE6063 mit Wert 67 vorhanden, muss STS mit Z32 vorhanden sein".into(),
        };
        let gr = AhbGroupRule {
            group_id: "SG10".into(),
            tag: "STS".into(),
            requirement: "C".into(),
            qualifier_restrictions: std::collections::BTreeMap::new(),
            conditional_rules: vec![cond],
            description: String::new(),
        };
        let mut out = String::new();
        emit_ahb_group_rule(&mut out, 13002, &gr);

        // Must emit a group-scoped closure, NOT a standalone fn
        assert!(
            out.contains("with_scoped_group_rule_fn"),
            "must use with_scoped_group_rule_fn: {out}"
        );
        assert!(
            out.contains("AHB-13002-SG10-STS-I0"),
            "rule_id format: {out}"
        );
        // Trigger uses `segs` (per-instance), not `segments` (flat)
        assert!(
            out.contains("segs.iter().any(|s| s.tag == \"QTY\""),
            "trigger uses per-instance segs: {out}"
        );
        // Consequence also uses `segs`
        assert!(
            out.contains("!segs.iter().any(|s| s.tag == \"STS\""),
            "consequence uses per-instance segs: {out}"
        );
        // Must annotate group_occurrence
        assert!(
            out.contains("group_occurrence"),
            "group_occurrence context: {out}"
        );
        // Error message must be group-scoped
        assert!(out.contains("in SG10:"), "group-qualified message: {out}");
    }
}
