//! Regression guards for the ORDERS Sperrung/Entsperrung AHB profile.
//!
//! Two defects were fixed here and both are cheap to re-break and expensive to
//! notice, so they are pinned:
//!
//! 1. PIDs 17008/17116/17117 were lost on import — only the first column of each
//!    multi-PID AHB table survived. A missing PID is silent: `ahb_rule_pack`
//!    returns an empty pack for an unknown PID, so validation passes everything.
//! 2. The `IMD` requirement was attributed one column to the left, marking it
//!    mandatory for 17115 (Sperrauftrag) instead of 17117 (Entsperrauftrag).

use std::fs;

fn segment_requirement(release: &str, pid: u32, tag: &str) -> Option<String> {
    let raw = fs::read_to_string(format!(
        "{}/profiles/orders/{release}/ahb.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("profile readable");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("profile parses");
    let entry = v["pruefidentifikatoren"]
        .as_array()?
        .iter()
        .find(|e| e["code"].as_u64() == Some(u64::from(pid)))?;
    entry["segment_rules"]
        .as_array()?
        .iter()
        .find(|r| r["tag"].as_str() == Some(tag))?["requirement"]
        .as_str()
        .map(ToOwned::to_owned)
}

/// Every Sperrung/Entsperrung PID must carry rules, not an empty pack.
///
/// 17116/17117 are routed by `mako-geli-gas::sperrung_nb::SPERRUNG_PIDS`, so an
/// empty pack disables AHB enforcement on that path without any error.
#[test]
fn sperrung_pids_have_rules_in_the_current_release() {
    for pid in [17007, 17008, 17115, 17116, 17117] {
        assert_eq!(
            segment_requirement("fv20261001", pid, "BGM").as_deref(),
            Some("M"),
            "PID {pid} must carry AHB rules in fv20260401"
        );
    }
}

/// `IMD` is required for the **Entsperrauftrag** (17117) only.
///
/// The AHB carries `IMD 00010 Muss` solely in the 17117 column — its `Z53`/`Z54`
/// values ("innerhalb/außerhalb der Arbeitszeit") only make sense for an
/// unblocking order. Both profiles previously gave that mark to 17115.
#[test]
fn imd_is_mandatory_only_for_the_entsperrauftrag() {
    for release in ["fv20260401", "fv20261001"] {
        assert_eq!(
            segment_requirement(release, 17117, "IMD").as_deref(),
            Some("M"),
            "{release}: 17117 Entsperrauftrag requires IMD"
        );
        assert_eq!(
            segment_requirement(release, 17115, "IMD").as_deref(),
            Some("O"),
            "{release}: 17115 Sperrauftrag must not require IMD"
        );
    }
}
