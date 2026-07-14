---
layout: default
title: Validation
nav_order: 11
parent: Reference
description: >
  Five-layer EDIFACT validation: schema, code lists, MIG, AHB, and semantic
  rules. How to read the ValidationReport, handle PIDs, and validate in
  strict mode.
---

# Validation Guide

Every EDI@Energy message can be validated against the officially registered BDEW profiles. This guide explains the validation model, how to read the report, and common patterns.

---

## The Five Validation Layers

| Layer | Checks |
|---|---|
| **1 — Schema** | Mandatory segments and data elements are present; repetition limits respected |
| **2 — Code lists** | Data element values are members of the permitted code list |
| **3 — MIG** | Message structure rules from the Marktkommunikation Implementation Guide (segment order, group nesting, cardinality) |
| **4 — AHB** | Pruefidentifikator-specific rules from the Anwendungshandbuch: mandatory/conditional/forbidden field presence |
| **5 — Semantic** | Cross-field business rules (date coherence, reference completeness, syntax acknowledgement validity) |

---

## Basic Validation

```rust
use edi_energy::{parse, EdiEnergyMessage};

let msg = parse(bytes)?;
let report = msg.validate()?;

if report.is_valid() {
    println!("OK");
} else {
    for issue in report.iter_issues() {
        println!("[{:?}] {}", issue.severity, issue.message);
    }
}
```

`validate()` uses the release detected from `UNH` and the Pruefidentifikator from `BGM` to select the correct profile automatically.

---

## Validating Against a Specific Pruefidentifikator

Use `validate_and_check_pid` when you want to assert the message is of a specific process type:

```rust
use edi_energy::{parse, validate_and_check_pid, Pruefidentifikator};

let msg = parse(bytes)?;
let pid = Pruefidentifikator::new(55001)?; // Lieferbeginn Strom
let report = validate_and_check_pid(&msg, pid)?;
report.into_error_result()?;
```

A PID mismatch does **not** return an `Err` — it returns `Ok(report)` where
`report.is_valid()` is `false` and `report.errors()` contains an issue with
rule ID `"EE-PID-001"`.  Check for this explicitly when you need to distinguish
a PID mismatch from other conformance failures:

```rust
let report = validate_and_check_pid(&msg, pid)?;
if !report.is_valid() {
    for issue in report.errors() {
        if issue.rule_id().as_deref() == Some("EE-PID-001") {
            eprintln!("PID mismatch: {}", issue.message);
        }
    }
}
```

---

## The Validation Report

`EdiEnergyReport` is the return value of all validate methods.

### Status checks

```rust
report.is_valid()       // true if no errors or critical issues
report.has_errors()     // true if any Error or Critical issues
report.has_warnings()   // true if any Warning issues
report.total_issues()   // total issue count across all severities
```

### Accessing issues

```rust
// All issues in order: errors → warnings → infos
for issue in report.iter_issues() { /* ... */ }

// Filtered by severity
for e in report.errors()    { /* ... */ }  // &[ValidationIssue]
for c in report.criticals() { /* ... */ }  // Iterator
for w in report.warnings()  { /* ... */ }  // &[ValidationIssue]
for i in report.infos()     { /* ... */ }  // &[ValidationIssue]

// Filtered by validation layer origin
// Values: "parse", "directory", "mig", "ahb", "custom"
for issue in report.issues_by_origin("ahb") { /* ... */ }

// Filtered by rule ID prefix (zero-allocation iterator)
for issue in report.issues_with_rule_prefix("AHB-55001-STS") { /* ... */ }

// Filtered by rule ID prefix (returns a new report for further chaining)
let ahb_report = report.filter_by_rule_prefix("AHB-55001");
```

### Converting to `Result`

```rust
// Ok(()) when no errors; Err(report) when errors are present (keeps the report)
let _ = report.into_result();   // returns Result<(), EdiEnergyReport>

// Ok(report) when valid; Err(Error::Validation { .. }) otherwise
report.as_result()?;

// Ok(()) when no errors; Err(Error::Validation { .. }) otherwise
report.into_error_result()?;
```

---

## `ValidationIssue` Fields

Each issue carries:

| Field | Type | Description |
|---|---|---|
| `severity` | `ValidationSeverity` | Critical / Error / Warning / Info |
| `message` | `&str` | Human-readable description |
| `rule_id` | `Option<&str>` | Stable rule identifier, e.g. `"AHB-55001-DTM-M0"` |
| `error_code` | `Option<&'static str>` | Machine-readable error code |
| `segment_tag` | `Option<String>` | EDIFACT segment tag, e.g. `"DTM"` |
| `segment_occurrence` | `Option<u16>` | 0-based occurrence index |
| `element_index` | `Option<u8>` | 0-based data-element index |
| `component_index` | `Option<u8>` | 0-based component index within a composite |
| `suggestion` | `Option<String>` | Suggested fix or explanation |

---

## Severity Levels

| Level | Meaning | `is_valid()` impact |
|---|---|---|
| `Critical` | Unrecoverable structural damage | ❌ Invalid |
| `Error` | Rule violation — message must not be processed | ❌ Invalid |
| `Warning` | Deviation — message should be reviewed | ✅ Valid (but flagged) |
| `Info` | Informational observation | ✅ Valid |

---

## Serializing Reports (`serde` feature)

Enable the `serde` feature to serialize reports as JSON:

```toml
edi-energy = { version = "0.9", features = ["serde"] }
```

```rust
use edi_energy::{parse, EdiEnergyMessage};

let msg    = parse(bytes)?;
let report = msg.validate()?;
let json   = serde_json::to_string_pretty(&report)?;
println!("{json}");
```

Output shape:

```json
{
  "valid": true,
  "issueCount": 0,
  "issues": []
}
```

---

## Rich Error Output (`diagnostics` feature)

Enable the `diagnostics` feature for `miette` integration:

```toml
edi-energy = { version = "0.9", features = ["diagnostics"] }
```

Reports then implement `miette::Diagnostic`, giving annotated terminal output with source spans when used with the `miette` error handler.

---

## Rule ID Naming Convention

Rule identifiers are stable, machine-readable strings generated by the
`cargo xtask codegen` pipeline from the BDEW profile JSON files.
They encode enough information to trace a fired rule back to its source.

### AHB rules (Application Handbook)

Format: `AHB-{PID}-[{SEGMENT_GROUP}-]{SEGMENT_TAG}-{QUALIFIER}{INDEX}`

| Component | Meaning | Example |
|---|---|---|
| `AHB` | Layer prefix — Application Handbook rule | `AHB` |
| `{PID}` | BDEW Prüfidentifikator | `55001`, `17102`, `55555` |
| `{SEGMENT_GROUP}` | Optional: segment group path (omitted for top-level segs) | `SG4`, `SG11` |
| `{SEGMENT_TAG}` | EDIFACT segment mnemonic | `DTM`, `STS`, `NAD`, `IMD` |
| `{QUALIFIER}` | Condition type: `I`=conditional-if, `M`=mandatory | `I`, `M` |
| `{INDEX}` | Zero-based occurrence counter (disambiguates multiple rules on the same segment) | `0`, `1`, `2` |

Examples:
```
AHB-55001-STS-I0          # PID 55001, STS segment (top-level), first conditional rule
AHB-55001-SG4-DTM-I0      # PID 55001, DTM inside SG4, first conditional rule
AHB-55002-SG4-FTX-I0      # PID 55002, FTX inside SG4, first conditional rule
AHB-17102-IMD-I0           # ORDERS PID 17102, IMD segment, first conditional rule
AHB-UNKNOWN-PID            # Fallback: fired when incoming PID has no AHB profile
```

**Locating the rule in the profile JSON**:

1. Find the profile: `crates/edi-energy/profiles/{msg_type}/fv{YYYYMMDD}/ahb.json`
2. Search for the `{PID}` in the top-level `"pruefidentifikatoren"` object.
3. Within the PID entry, look for the segment group path (e.g. `"SG4"`) and
   segment tag (e.g. `"DTM"`).
4. The `"qualifier"` and `"condition"` fields of the matching entry generate
   the `{QUALIFIER}{INDEX}` suffix via codegen.

Example profile path for `AHB-55001-SG4-DTM-I0`:
```
crates/edi-energy/profiles/utilmd/fv20250606/ahb.json
  → pruefidentifikatoren → "55001" → "SG4" → "DTM"
```

### MIG rules (Message Implementation Guide)

Format: `MIG-{SEGMENT_TAG}-REQ` or `MIG-{MSG_TYPE}-{MIG_VERSION}-GROUP-{SG}-{SEG}-CARD-{BOUND}`

| Pattern | Meaning | Example |
|---|---|---|
| `MIG-{SEG}-REQ` | Mandatory segment absent | `MIG-BGM-REQ`, `MIG-DTM-REQ` |
| `MIG-{MSG}-{VER}-GROUP-{SG}-{SEG}-CARD-MAX` | Maximum cardinality exceeded | `MIG-ORDERS-MIG-1.4b-GROUP-SG2-NAD-CARD-MAX` |
| `MIG-{MSG}-{VER}-GROUP-{SG}-{SEG}-CARD-MIN` | Minimum cardinality not met | `MIG-ORDERS-MIG-1.4b-GROUP-SG1-RFF-CARD-MIN` |

### Semantic and engine rules

| Prefix | Layer | Example |
|---|---|---|
| `SEM-` | Cross-segment semantic check | `SEM-UTILMD-DATE-COH` |
| `EE-` | Engine-level check (e.g. PID check) | `EE-PID-001` |

### Filtering by rule prefix

```rust
// All AHB rules for PID 55001 (zero-allocation iterator)
for issue in report.issues_with_rule_prefix("AHB-55001") { /* ... */ }

// All AHB rules inside SG4 for PID 55001
for issue in report.issues_with_rule_prefix("AHB-55001-SG4") { /* ... */ }

// All MIG rules for DTM
for issue in report.issues_with_rule_prefix("MIG-DTM") { /* ... */ }

// All AHB rules regardless of PID
for issue in report.issues_with_rule_prefix("AHB-") { /* ... */ }

// By validation layer: "parse", "directory", "mig", "ahb", "custom"
for issue in report.issues_by_origin("mig") { /* ... */ }
```

---

## Validate Against a Specific Release Date

To reproduce past validation behaviour or run tests against historical data:

```rust
use edi_energy::{parse_with_config, ParseConfig, EdiEnergyMessage};
use time::macros::date;

let config = ParseConfig::new()
    .with_reference_date(date!(2024-10-01));

let msg = parse_with_config(bytes, config)?;
let report = msg.validate()?;
// profile selection uses Oct 1 2024 as "today"
```

---

## AHB Conditional Rules Explained

AHB rules specify whether a field is:

| Code | Meaning |
|---|---|
| `M` | Mandatory — must be present |
| `C` | Conditional — presence depends on another field |
| `N` | Not used — must be absent |
| `O` | Optional — may be present |
| `S` | Situational — present in some process variants |
| `X` | Exclusive — used in exclusive-or conditions |

Conditional (`C`) rules have an associated expression such as:

> "Must be present when STS DE 0061 qualifies the state as active"

The AHB profiles encode these as structured `ConditionalRule` objects with `operator`, `tag`, and optional `secondary_tag` fields.

---

## See Also

- [Parsing Guide](./parsing.md)
- [Builder Guide](./builders.md)
- [Release Lifecycle](./release-lifecycle.md)
