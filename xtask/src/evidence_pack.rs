//! `cargo xtask evidence-pack` — the per-release conformance evidence pack.
//!
//! One JSON document per release, `evidence-v<version>.json`, published as a
//! GitHub Release asset: what the release conforms to, in figures a third
//! party can check, beside the SBOMs and the VEX document.
//!
//! ```text
//! cargo xtask evidence-pack --inputs <dir> --out <file> [--version X.Y.Z] [--commit <sha>]
//! cargo xtask evidence-pack --check <file>
//! cargo xtask evidence-pack --xtask-recipes
//! ```
//!
//! The release workflow collects each producer's output into `<dir>`; this
//! task reads nothing else from the outside world. Every input is optional:
//!
//! | input in `<dir>`          | section             |
//! |---------------------------|---------------------|
//! | `fidelity.log`            | `fidelity` — stdout of `extraction_fidelity`'s two budget tests |
//! | `outbound-sweep.json`     | `outbound_sweep`    |
//! | `guards/<recipe>.exit`    | `guards` — one exit status per recipe of `just ci` |
//! | `mutation-score.json`     | `mutation_score`    |
//! | `lead-time.json`          | `release_lead_time` |
//! | `sbom/*.cdx.json`         | `sbom` — SHA-256 per CycloneDX document |
//! | `attestations/*.json`     | `attestations` — one record per attested subject |
//!
//! **A missing input is `not measured`, never a zero.** A section whose
//! producer did not run, or whose output this task cannot read, says so in
//! `status` and carries no figure at all — a `0` in its place would read as a
//! perfect score. `--check` refuses a pack with a section absent, a status
//! other than the two, a `measured` section without its figures, or a guard
//! that ran and failed.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Every section a pack carries, in order. `--check` requires each.
pub const SECTIONS: &[&str] = &[
    "fidelity",
    "outbound_sweep",
    "guards",
    "mutation_score",
    "release_lead_time",
    "sbom",
    "attestations",
];

const MEASURED: &str = "measured";
const NOT_MEASURED: &str = "not measured";

const FIDELITY_TEST: &str = "crates/edi-energy/tests/it/extraction_fidelity.rs";

pub fn run(workspace_root: &Path, args: &[String]) -> bool {
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--xtask-recipes") {
        let justfile = std::fs::read_to_string(workspace_root.join("justfile")).unwrap_or_default();
        for r in xtask_recipes(&justfile) {
            println!("{r}");
        }
        return true;
    }
    if let Some(file) = arg("--check") {
        let Ok(text) = std::fs::read_to_string(&file) else {
            eprintln!("evidence-pack: cannot read {file} — a release without its pack is refused");
            return false;
        };
        let findings = match serde_json::from_str::<Value>(&text) {
            Ok(pack) => check(&pack),
            Err(e) => vec![format!("not JSON: {e}")],
        };
        if findings.is_empty() {
            println!(
                "evidence-pack: {file} carries all {} sections",
                SECTIONS.len()
            );
            return true;
        }
        eprintln!("evidence-pack: {file} is refused:");
        for f in &findings {
            eprintln!("  {f}");
        }
        return false;
    }
    let (Some(inputs), Some(out)) = (arg("--inputs"), arg("--out")) else {
        eprintln!(
            "usage: cargo xtask evidence-pack --inputs <dir> --out <file> [--version X.Y.Z] \
             [--commit <sha>]\n       cargo xtask evidence-pack --check <file>\n       \
             cargo xtask evidence-pack --xtask-recipes"
        );
        return false;
    };
    let version = arg("--version").or_else(|| workspace_version(workspace_root));
    let commit = arg("--commit")
        .or_else(|| std::env::var("GITHUB_SHA").ok())
        .or_else(|| command_line("git", &["rev-parse", "HEAD"]));
    let toolchain = command_line("rustc", &["--version"]);
    match assemble(
        workspace_root,
        Path::new(&inputs),
        version.as_deref(),
        commit.as_deref(),
        toolchain.as_deref(),
    ) {
        Ok(pack) => {
            let text = serde_json::to_string_pretty(&pack).expect("a JSON value serialises");
            if let Err(e) = std::fs::write(&out, text + "\n") {
                eprintln!("evidence-pack: cannot write {out}: {e}");
                return false;
            }
            let unmeasured: Vec<&str> = SECTIONS
                .iter()
                .copied()
                .filter(|s| pack[*s]["status"] == NOT_MEASURED)
                .collect();
            println!(
                "evidence-pack: wrote {out}; not measured: {}",
                if unmeasured.is_empty() {
                    "none".to_string()
                } else {
                    unmeasured.join(", ")
                }
            );
            true
        }
        Err(e) => {
            eprintln!("evidence-pack: {e}");
            false
        }
    }
}

fn command_line(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn workspace_version(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_pkg = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[workspace.package]";
            continue;
        }
        if in_pkg {
            if let Some(v) = t.strip_prefix("version") {
                return Some(
                    v.trim_start_matches([' ', '='])
                        .trim()
                        .trim_matches('"')
                        .to_string(),
                );
            }
        }
    }
    None
}

fn not_measured(why: &str) -> Value {
    json!({ "status": NOT_MEASURED, "reason": why })
}

/// Build the pack from the inputs directory.
pub fn assemble(
    root: &Path,
    inputs: &Path,
    version: Option<&str>,
    commit: Option<&str>,
    toolchain: Option<&str>,
) -> Result<Value, String> {
    let fidelity_src = std::fs::read_to_string(root.join(FIDELITY_TEST))
        .map_err(|e| format!("cannot read {FIDELITY_TEST}: {e}"))?;
    let justfile = std::fs::read_to_string(root.join("justfile"))
        .map_err(|e| format!("cannot read justfile: {e}"))?;
    let recipes = ci_recipes(&justfile);
    if recipes.is_empty() {
        return Err("the justfile has no `ci:` recipe — the guard list is unreadable".into());
    }

    let mut pack = serde_json::Map::new();
    pack.insert("version".into(), json!(version));
    pack.insert("commit".into(), json!(commit));
    pack.insert("toolchain".into(), json!(toolchain));
    pack.insert(
        "fidelity".into(),
        fidelity(
            &fidelity_src,
            std::fs::read_to_string(inputs.join("fidelity.log")).ok(),
        )?,
    );
    pack.insert(
        "outbound_sweep".into(),
        opaque(inputs, "outbound-sweep.json"),
    );
    pack.insert("guards".into(), guards(&recipes, &inputs.join("guards")));
    pack.insert(
        "mutation_score".into(),
        opaque(inputs, "mutation-score.json"),
    );
    pack.insert("release_lead_time".into(), opaque(inputs, "lead-time.json"));
    pack.insert("sbom".into(), sbom(&inputs.join("sbom")));
    pack.insert(
        "attestations".into(),
        attestations(&inputs.join("attestations")),
    );
    Ok(Value::Object(pack))
}

/// `const <NAME>: usize = <n>;` out of the fidelity test's source.
fn budget(src: &str, name: &str) -> Option<u64> {
    let pat = format!("const {name}: usize = ");
    let rest = &src[src.find(&pat)? + pat.len()..];
    rest[..rest.find(';')?].trim().replace('_', "").parse().ok()
}

/// The number after `label` on the line that carries it in the test output.
fn figure_after(log: &str, label: &str) -> Option<u64> {
    let line = log.lines().find(|l| l.contains(label))?;
    let rest = &line[line.find(label)? + label.len()..];
    let digits: String = rest
        .trim_start_matches([' ', ':'])
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn fidelity(src: &str, log: Option<String>) -> Result<Value, String> {
    let damage = budget(src, "DAMAGE_BUDGET")
        .ok_or_else(|| format!("{FIDELITY_TEST} declares no DAMAGE_BUDGET"))?;
    let silent = budget(src, "SILENT_PERMIT_BUDGET")
        .ok_or_else(|| format!("{FIDELITY_TEST} declares no SILENT_PERMIT_BUDGET"))?;
    let budgets = json!({ "DAMAGE_BUDGET": damage, "SILENT_PERMIT_BUDGET": silent });
    let Some(log) = log else {
        let mut v = not_measured("fidelity.log absent: the budget tests did not run");
        v["budgets"] = budgets;
        return Ok(v);
    };
    let measured_damage = figure_after(&log, "distinct conditions");
    let measured_silent = figure_after(&log, "binding places gated by an unreadable Voraussetzung");
    match (measured_damage, measured_silent) {
        (Some(d), Some(s)) => Ok(json!({
            "status": MEASURED,
            "budgets": budgets,
            "measured": { "DAMAGE_BUDGET": d, "SILENT_PERMIT_BUDGET": s },
            "within_budget": d <= damage && s <= silent,
        })),
        _ => {
            let mut v = not_measured(
                "fidelity.log does not carry both measured figures — the test output changed shape",
            );
            v["budgets"] = budgets;
            Ok(v)
        }
    }
}

/// A producer's JSON output carried as-is, or `not measured` without it.
fn opaque(inputs: &Path, file: &str) -> Value {
    let Ok(text) = std::fs::read_to_string(inputs.join(file)) else {
        return not_measured(&format!("{file} absent: its producer did not run"));
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(v) if !v.is_null() => json!({ "status": MEASURED, "values": v }),
        _ => not_measured(&format!("{file} is not a JSON value")),
    }
}

/// The recipes `just ci` runs, in the order the recipe names them: the
/// dependencies of the private `_ci` recipe `ci` delegates to, or of `ci`
/// itself where it lists them directly.
pub fn ci_recipes(justfile: &str) -> Vec<String> {
    let deps_of = |name: &str| {
        justfile
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .map(|deps| {
                deps.split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|d| !d.is_empty())
    };
    deps_of("_ci:")
        .or_else(|| deps_of("ci:"))
        .unwrap_or_default()
}

/// The xtask arguments of one recipe-body line, in any form that runs the
/// `xtask` binary: `cargo xtask <args>` (the `.cargo/config.toml` alias),
/// `cargo run [flags] -p xtask -- <args>` or `--package xtask`.
pub fn xtask_invocation(line: &str) -> Option<&str> {
    let line = line.trim();
    if let Some(args) = line.strip_prefix("cargo xtask ") {
        return Some(args.trim()).filter(|a| !a.is_empty());
    }
    let rest = line.strip_prefix("cargo run ")?;
    let (flags, args) = rest.split_once(" -- ")?;
    let flags: Vec<&str> = flags.split_whitespace().collect();
    let names_xtask = flags
        .windows(2)
        .any(|w| matches!(w[0], "-p" | "--package") && w[1] == "xtask")
        || flags
            .iter()
            .any(|f| *f == "--package=xtask" || *f == "-pxtask");
    names_xtask.then_some(args.trim()).filter(|a| !a.is_empty())
}

/// Every recipe body in the justfile: name → its non-comment lines.
pub fn recipe_bodies(justfile: &str) -> BTreeMap<String, Vec<String>> {
    let mut bodies: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in justfile.lines() {
        if line.starts_with(char::is_whitespace) {
            if let Some(name) = &current {
                let t = line.trim();
                if !t.is_empty() && !t.starts_with('#') {
                    bodies.entry(name.clone()).or_default().push(t.to_string());
                }
            }
            continue;
        }
        current = line
            .split_once(':')
            .filter(|(name, rest)| {
                !name.is_empty()
                    && !rest.starts_with('=')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            })
            .map(|(name, _)| name.to_string());
    }
    bodies
}

/// The `ci` recipes whose whole body is one call of the `xtask` binary — the
/// ones a job can run without building the workspace.
pub fn xtask_recipes(justfile: &str) -> Vec<String> {
    let bodies = recipe_bodies(justfile);
    ci_recipes(justfile)
        .into_iter()
        .filter(|r| {
            bodies
                .get(r)
                .is_some_and(|b| b.len() == 1 && xtask_invocation(&b[0]).is_some())
        })
        .collect()
}

fn guards(recipes: &[String], dir: &Path) -> Value {
    let mut measured = 0usize;
    let list: Vec<Value> = recipes
        .iter()
        .map(|r| {
            let exit = std::fs::read_to_string(dir.join(format!("{r}.exit")))
                .ok()
                .and_then(|t| t.trim().parse::<i32>().ok());
            match exit {
                Some(code) => {
                    measured += 1;
                    json!({ "recipe": r, "status": MEASURED, "exit": code })
                }
                None => json!({ "recipe": r, "status": NOT_MEASURED }),
            }
        })
        .collect();
    if measured == 0 {
        let mut v = not_measured("no guard verdicts were recorded");
        v["recipes"] = Value::Array(list);
        return v;
    }
    json!({
        "status": MEASURED,
        "measured": measured,
        "of": recipes.len(),
        "recipes": list,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn sbom(dir: &Path) -> Value {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".cdx.json"))
        .collect();
    files.sort();
    let docs: Vec<Value> = files
        .iter()
        .filter_map(|p| {
            let bytes = std::fs::read(p).ok()?;
            Some(json!({
                "file": p.file_name()?.to_string_lossy(),
                "sha256": sha256_hex(&bytes),
            }))
        })
        .collect();
    if docs.is_empty() {
        return not_measured("no CycloneDX document (sbom/*.cdx.json) was collected");
    }
    json!({ "status": MEASURED, "documents": docs })
}

fn attestations(dir: &Path) -> Value {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let records: Vec<Value> = files
        .iter()
        .filter_map(|p| serde_json::from_str::<Value>(&std::fs::read_to_string(p).ok()?).ok())
        .filter(|v| v.is_object())
        .collect();
    if records.is_empty() {
        return not_measured("no attestation record (attestations/*.json) was collected");
    }
    json!({ "status": MEASURED, "records": records })
}

/// Every reason a pack is refused.
pub fn check(pack: &Value) -> Vec<String> {
    let mut findings = Vec::new();
    for key in ["version", "commit", "toolchain"] {
        if pack[key].as_str().is_none_or(str::is_empty) {
            findings.push(format!("`{key}` is missing"));
        }
    }
    for s in SECTIONS {
        let Some(section) = pack.get(*s) else {
            findings.push(format!(
                "section `{s}` is absent — say `not measured`, never omit"
            ));
            continue;
        };
        match section["status"].as_str() {
            Some(NOT_MEASURED) => {
                if section["reason"].as_str().is_none_or(str::is_empty) {
                    findings.push(format!("`{s}` is not measured and says not why"));
                }
            }
            Some(MEASURED) => {
                let carries = match *s {
                    "fidelity" => section["measured"].is_object(),
                    "guards" => section["recipes"].as_array().is_some_and(|a| !a.is_empty()),
                    "sbom" => section["documents"]
                        .as_array()
                        .is_some_and(|a| !a.is_empty()),
                    "attestations" => section["records"].as_array().is_some_and(|a| !a.is_empty()),
                    _ => !section["values"].is_null(),
                };
                if !carries {
                    findings.push(format!("`{s}` says measured and carries no figure"));
                }
            }
            other => findings.push(format!(
                "`{s}` has status {other:?}; it is `{MEASURED}` or `{NOT_MEASURED}`"
            )),
        }
    }
    if pack["fidelity"]["within_budget"] == Value::Bool(false) {
        findings.push("`fidelity` is over budget".into());
    }
    for r in pack["guards"]["recipes"].as_array().into_iter().flatten() {
        if let Some(code) = r["exit"].as_i64() {
            if code != 0 {
                findings.push(format!(
                    "guard `{}` exited {code} — a release does not ship a failing guard",
                    r["recipe"].as_str().unwrap_or("?")
                ));
            }
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_recipes_follow_the_private_gate() {
        let j = "ci:\n    RUSTFLAGS=x just _ci\n\n_ci: check check-vex\n";
        assert_eq!(ci_recipes(j), ["check", "check-vex"]);
        assert_eq!(ci_recipes("ci: check deny\n"), ["check", "deny"]);
    }

    const SRC: &str =
        "const DAMAGE_BUDGET: usize = 46;\nconst SILENT_PERMIT_BUDGET: usize = 2203;\n";
    const JUSTFILE: &str = "\
ci: check check-vex check-licenses

check:
    cargo check --all-targets

check-vex:
    cargo xtask check-vex

check-licenses:
    cargo xtask check-licenses
";
    const LOG: &str = "\
running 2 tests
distinct conditions     : 44
binding places gated by an unreadable Voraussetzung: 2100 of 24000, in 300 of the shipped Anwendungsfälle. Worst:
";

    /// A scratch workspace with the two files `assemble` reads from the tree,
    /// and an inputs directory with every producer's output.
    fn fixture(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("evidence-pack-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(FIDELITY_TEST).parent().unwrap()).unwrap();
        std::fs::write(root.join(FIDELITY_TEST), SRC).unwrap();
        std::fs::write(root.join("justfile"), JUSTFILE).unwrap();
        let inputs = root.join("inputs");
        for d in ["guards", "sbom", "attestations"] {
            std::fs::create_dir_all(inputs.join(d)).unwrap();
        }
        std::fs::write(inputs.join("fidelity.log"), LOG).unwrap();
        std::fs::write(inputs.join("outbound-sweep.json"), r#"{"55001":"ok"}"#).unwrap();
        std::fs::write(inputs.join("mutation-score.json"), r#"{"score":0.81}"#).unwrap();
        std::fs::write(
            inputs.join("lead-time.json"),
            r#"[{"profile":"x","days":3}]"#,
        )
        .unwrap();
        for g in ["check", "check-vex", "check-licenses"] {
            std::fs::write(inputs.join("guards").join(format!("{g}.exit")), "0\n").unwrap();
        }
        std::fs::write(inputs.join("sbom/makod_bin.cdx.json"), "{}").unwrap();
        std::fs::write(
            inputs.join("attestations/makod.json"),
            r#"{"subject":"ghcr.io/o/mako-makod","digest":"sha256:00"}"#,
        )
        .unwrap();
        (root, inputs)
    }

    fn pack(root: &Path, inputs: &Path) -> Value {
        assemble(
            root,
            inputs,
            Some("0.22.0"),
            Some("abc"),
            Some("rustc 1.94.1"),
        )
        .unwrap()
    }

    #[test]
    fn a_complete_input_set_is_measured_everywhere_and_passes_the_check() {
        let (root, inputs) = fixture("full");
        let p = pack(&root, &inputs);
        std::fs::remove_dir_all(&root).ok();
        for s in SECTIONS {
            assert_eq!(p[*s]["status"], MEASURED, "{s}: {}", p[*s]);
        }
        assert_eq!(p["fidelity"]["measured"]["DAMAGE_BUDGET"], 44);
        assert_eq!(p["fidelity"]["measured"]["SILENT_PERMIT_BUDGET"], 2100);
        assert_eq!(p["fidelity"]["budgets"]["SILENT_PERMIT_BUDGET"], 2203);
        assert!(check(&p).is_empty(), "{:?}", check(&p));
    }

    #[test]
    fn each_deleted_input_reads_not_measured_and_never_zero() {
        let deletions: &[(&str, &str)] = &[
            ("fidelity.log", "fidelity"),
            ("outbound-sweep.json", "outbound_sweep"),
            ("mutation-score.json", "mutation_score"),
            ("lead-time.json", "release_lead_time"),
            ("guards", "guards"),
            ("sbom", "sbom"),
            ("attestations", "attestations"),
        ];
        for (input, section) in deletions {
            let (root, inputs) = fixture(section);
            let path = inputs.join(input);
            if path.is_dir() {
                std::fs::remove_dir_all(&path).unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let p = pack(&root, &inputs);
            std::fs::remove_dir_all(&root).ok();
            let s = &p[*section];
            assert_eq!(s["status"], NOT_MEASURED, "{section}: {s}");
            assert!(s["reason"].as_str().is_some_and(|r| !r.is_empty()));
            for figure in ["measured", "values", "documents", "records", "score"] {
                assert!(
                    s[figure].is_null(),
                    "{section} carries `{figure}` while not measured"
                );
            }
            // Still a valid pack: not measured is a statement, not a hole.
            assert!(check(&p).is_empty(), "{section}: {:?}", check(&p));
        }
    }

    #[test]
    fn a_guard_without_a_verdict_is_not_measured_on_its_own() {
        let (root, inputs) = fixture("one-guard");
        std::fs::remove_file(inputs.join("guards/check-vex.exit")).unwrap();
        let p = pack(&root, &inputs);
        std::fs::remove_dir_all(&root).ok();
        let r = &p["guards"]["recipes"];
        assert_eq!(r[1]["recipe"], "check-vex");
        assert_eq!(r[1]["status"], NOT_MEASURED);
        assert!(r[1]["exit"].is_null());
        assert_eq!(p["guards"]["measured"], 2);
    }

    #[test]
    fn the_check_refuses_an_absent_section_a_failed_guard_and_an_empty_measurement() {
        let (root, inputs) = fixture("refuse");
        std::fs::write(inputs.join("guards/check-vex.exit"), "1").unwrap();
        let mut p = pack(&root, &inputs);
        std::fs::remove_dir_all(&root).ok();
        p.as_object_mut().unwrap().remove("sbom");
        p["outbound_sweep"] = json!({ "status": MEASURED });
        p["mutation_score"] = json!({ "status": "0" });
        let f = check(&p);
        assert_eq!(f.len(), 4, "{f:?}");
        assert!(f.iter().any(|x| x.contains("`sbom` is absent")));
        assert!(f.iter().any(|x| x.contains("check-vex")));
    }

    #[test]
    fn only_single_xtask_call_recipes_are_offered_to_the_release_job() {
        assert_eq!(xtask_recipes(JUSTFILE), ["check-vex", "check-licenses"]);
    }

    #[test]
    fn every_form_of_an_xtask_call_is_an_xtask_recipe() {
        let j = "\
_ci: a b c d e

a:
    cargo run -q -p xtask -- check-answer-commands

b:
    cargo run --package xtask -- check-vex

c:
    cargo run -q -p other -- check-vex

d:
    cargo xtask sync-regulatories --offline

e:
    cargo xtask check-vex
    cargo test
";
        assert_eq!(xtask_recipes(j), ["a", "b", "d"]);
        assert_eq!(
            xtask_invocation("cargo run -q -p xtask -- check-answer-commands"),
            Some("check-answer-commands")
        );
        assert_eq!(xtask_invocation("cargo xtask"), None);
    }

    #[test]
    fn every_xtask_recipe_of_the_real_justfile_gets_a_verdict() {
        let justfile =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../justfile"))
                .unwrap();
        let recipes = xtask_recipes(&justfile);
        assert!(
            recipes.iter().any(|r| r == "check-answer-commands"),
            "{recipes:?}"
        );
    }
}
