//! Guard: every operator policy setting is sourced, documented, generated,
//! consulted, and the only way a config field silently defaults an enum.
//!
//! A reading the operator chooses must be visible and attributable: which
//! value ran, whether someone set it, and the primary source of that value.
//! `mako_service::policy_choice!` carries all of it, so the failure modes are
//! the ones a declaration can still have:
//!
//! - **an option without a source**, or with one `check-citations` or
//!   `check-known-wrong` refuses — the setting would print an unsourced or a
//!   known-wrong `§` into every startup log and result trace;
//! - **a `doc` anchor that does not resolve** — the report points an operator
//!   at nothing;
//! - **a generated page out of sync** — `operator-policy.md` lists a setting
//!   that is gone, or misses one that is new;
//! - **a tightening default without its reason** — the direction is the claim,
//!   and a claim without its reason cannot be checked;
//! - **a required setting without a refusal test** — „refuses to start" is the
//!   whole of the contract, and nothing shows it holds;
//! - **a key no production code reads** — a setting that is printed and never
//!   consulted is the „configured but never enforced" shape;
//! - **a `#[serde(default)]` enum field in a service config that is not a
//!   policy** — the defaulted value is taken silently, and nothing records that
//!   it was a default. Such a field is either a `Policy<T>` or carries a
//!   reasoned [`NOT_POLICY`] entry.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::policy_inventory::{self, PolicyDecl, PolicyDefault};

/// `#[serde(default)]` enum fields in a service config that are not declared
/// as operator policy, each with the reason. `service/field` → reason.
pub const NOT_POLICY: &[(&str, &str)] = &[
    (
        "agentd/journal",
        "a storage backend for the agent journal — where records are written, not a reading of \
         any source",
    ),
    (
        "invoicd/marktrolle",
        "the Marktrolle this deployment receives invoices in — a fact of the deployment \
         (Identifikatoren AWH V1.2 Kap. 2.1: one MP-ID per Rolle), not a reading of a source",
    ),
    (
        "netzbilanzd/submission_target",
        "the transport Summenzeitreihen are filed over; a target that is not implemented is \
         refused at startup",
    ),
    (
        "productd/behg_regime",
        "an operator reading, the same one as billingd `[policy] behg_regime`; productd \
         prices quotations with it and is not yet declared through policy_choice!, so this \
         entry is the open item and is removed when productd declares it",
    ),
];

/// One finding: where, and what is wrong.
type Finding = (String, String);

pub fn run(root: &Path) -> bool {
    let decls = policy_inventory::collect(root);
    if decls.is_empty() {
        eprintln!(
            "check-policy-settings: found no policy_choice! declaration — has the layout changed?"
        );
        return false;
    }
    let mut findings = Vec::new();
    for d in &decls {
        findings.extend(declaration_findings(d));
        findings.extend(doc_findings(root, d));
        findings.extend(read_findings(root, d));
        findings.extend(required_test_findings(root, d));
    }
    let page = std::fs::read_to_string(root.join(policy_inventory::PAGE)).unwrap_or_default();
    if page != policy_inventory::render(&decls) {
        findings.push((
            policy_inventory::PAGE.to_owned(),
            "differs from the policy_choice! declarations — run `cargo xtask policy-inventory`"
                .to_owned(),
        ));
    }
    let policy_enums: BTreeSet<String> = decls.iter().map(|d| d.enum_name.clone()).collect();
    let enums = workspace_enums(root);
    for (file, src) in service_configs(root) {
        findings.extend(defaulted_enum_findings(
            &file,
            &src,
            &enums,
            &policy_enums,
            NOT_POLICY,
        ));
    }
    if findings.is_empty() {
        println!(
            "check-policy-settings: {} setting(s) sourced, documented, generated and consulted; \
             no service config defaults an enum outside a policy",
            decls.len()
        );
        return true;
    }
    eprintln!("check-policy-settings: {} finding(s):", findings.len());
    for (at, msg) in &findings {
        eprintln!("  {at}: {msg}");
    }
    false
}

fn at(d: &PolicyDecl) -> String {
    format!("{}:{} ({})", d.file, d.line, d.key)
}

/// Sources, citations, reasons — what one declaration says about itself.
#[must_use]
pub fn declaration_findings(d: &PolicyDecl) -> Vec<Finding> {
    let mut out = Vec::new();
    if d.key.is_empty() {
        out.push((at(d), "declares no `key`".to_owned()));
    }
    if d.options.is_empty() {
        out.push((at(d), "declares no option".to_owned()));
    }
    for o in &d.options {
        if o.quelle.trim().is_empty() {
            out.push((
                at(d),
                format!("option `{}` names no primary source", o.name),
            ));
            continue;
        }
        for (_, reason) in crate::check_citations::offending_lines_in("", &o.quelle) {
            out.push((at(d), format!("option `{}`: {reason}", o.name)));
        }
        for (_, msg) in crate::check_known_wrong::findings_in_text(&o.quelle) {
            out.push((at(d), format!("option `{}`: {msg}", o.name)));
        }
    }
    match &d.default {
        PolicyDefault::Tightening { variant, reason } => {
            if reason.trim().is_empty() {
                out.push((
                    at(d),
                    "a tightening default states no reason it cannot breach".to_owned(),
                ));
            }
            if !d.options.iter().any(|o| &o.variant == variant) {
                out.push((at(d), format!("the default `{variant}` is no option")));
            }
        }
        PolicyDefault::Required => {}
    }
    out
}

/// The `doc` anchor names a page under `site/content` that carries an
/// explicit `{#anchor}` heading id.
fn doc_findings(root: &Path, d: &PolicyDecl) -> Vec<Finding> {
    let Some(rest) = d.doc.strip_prefix("@/") else {
        return vec![(at(d), format!("doc `{}` is not an `@/`-rooted link", d.doc))];
    };
    let Some((page, anchor)) = rest.split_once('#') else {
        return vec![(at(d), format!("doc `{}` names no anchor", d.doc))];
    };
    let Ok(text) = std::fs::read_to_string(root.join("site/content").join(page)) else {
        return vec![(at(d), format!("doc page `{page}` does not exist"))];
    };
    if text.contains(&format!("{{#{anchor}}}")) {
        Vec::new()
    } else {
        vec![(
            at(d),
            format!("doc page `{page}` has no heading with the id `{{#{anchor}}}`"),
        )]
    }
}

/// Production source of a service: every `.rs` under `src`, with the
/// `#[cfg(test)]` tail of each file cut off.
fn production_sources(root: &Path, service: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    rust_files(&root.join("services").join(service).join("src"), &mut files);
    files
        .into_iter()
        .filter_map(|f| {
            let src = std::fs::read_to_string(&f).ok()?;
            let cut = src.find("#[cfg(test)]").unwrap_or(src.len());
            Some((f.to_string_lossy().into_owned(), src[..cut].to_owned()))
        })
        .collect()
}

/// The setting is consulted: its enum (a variant comparison) or its field
/// (`.key` or `.key(`) appears in production code outside its own declaration,
/// its `Policy<…>` field, an `impl` header and the report line.
fn read_findings(root: &Path, d: &PolicyDecl) -> Vec<Finding> {
    let field = d.key.rsplit('.').next().unwrap_or(&d.key).to_owned();
    let accessor = format!(".{field}");
    for (_, src) in production_sources(root, &d.service) {
        let mut in_macro = false;
        for line in src.lines() {
            let t = line.trim();
            if t.ends_with("policy_choice! {") {
                in_macro = true;
            }
            if in_macro {
                if line.starts_with('}') {
                    in_macro = false;
                }
                continue;
            }
            if t.starts_with("//")
                || t.starts_with("impl")
                || t.contains("Policy<")
                || t.contains(".with(")
            {
                continue;
            }
            if t.contains(&d.enum_name) || t.contains(&accessor) {
                return Vec::new();
            }
        }
    }
    vec![(
        at(d),
        format!(
            "no production code in services/{} reads the setting (neither `{}` nor `{accessor}` \
             outside its declaration and its report line) — a setting that is printed and never \
             consulted is a control that looks configured and is not",
            d.service, d.enum_name
        ),
    )]
}

/// A `Required` setting has a test showing its absence refuses: a test file
/// or test module of the service naming the key beside `PolicyRequired` or
/// the loader's „missing policy setting".
fn required_test_findings(root: &Path, d: &PolicyDecl) -> Vec<Finding> {
    if d.default != PolicyDefault::Required {
        return Vec::new();
    }
    let svc = root.join("services").join(&d.service);
    let mut files = Vec::new();
    rust_files(&svc.join("src"), &mut files);
    rust_files(&svc.join("tests"), &mut files);
    let shown = files.iter().any(|f| {
        std::fs::read_to_string(f).is_ok_and(|src| {
            let tests = src.find("#[cfg(test)]").map_or(
                if f.components().any(|c| c.as_os_str() == "tests") {
                    src.as_str()
                } else {
                    ""
                },
                |i| &src[i..],
            );
            tests.contains(&format!("\"{}\"", d.key))
                && (tests.contains("PolicyRequired") || tests.contains("missing policy setting"))
        })
    });
    if shown {
        Vec::new()
    } else {
        vec![(
            at(d),
            "a required setting with no test showing that its absence refuses startup".to_owned(),
        )]
    }
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every `enum` name declared under `services/*/src` and `crates/*/src`.
fn workspace_enums(root: &Path) -> BTreeSet<String> {
    let mut files = Vec::new();
    for top in ["services", "crates"] {
        let Ok(dirs) = std::fs::read_dir(root.join(top)) else {
            continue;
        };
        for d in dirs.flatten() {
            rust_files(&d.path().join("src"), &mut files);
        }
    }
    let mut out = BTreeSet::new();
    for f in files {
        let Ok(src) = std::fs::read_to_string(&f) else {
            continue;
        };
        out.extend(enum_names(&src));
    }
    out
}

fn enum_names(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|l| {
            let t = l.trim_start();
            let rest = t
                .strip_prefix("pub enum ")
                .or_else(|| t.strip_prefix("pub(crate) enum "))
                .or_else(|| t.strip_prefix("enum "))?;
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// Every `config.rs` under `services/*/src`, workspace-relative.
fn service_configs(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(root.join("services")) else {
        return out;
    };
    let mut files = Vec::new();
    for d in dirs.flatten() {
        rust_files(&d.path().join("src"), &mut files);
    }
    files.sort();
    for f in files {
        if f.file_name().is_none_or(|n| n != "config.rs") {
            continue;
        }
        if let Ok(src) = std::fs::read_to_string(&f) {
            let rel = f
                .strip_prefix(root)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, src));
        }
    }
    out
}

/// A `#[serde(default…)]` field whose type is a bare workspace enum that is
/// neither a policy setting nor listed in `allowed` as `service/field`.
#[must_use]
pub fn defaulted_enum_findings(
    file: &str,
    src: &str,
    enums: &BTreeSet<String>,
    policy_enums: &BTreeSet<String>,
    allowed: &[(&str, &str)],
) -> Vec<Finding> {
    let service = file
        .strip_prefix("services/")
        .and_then(|r| r.split('/').next())
        .unwrap_or("");
    let cut = src.find("#[cfg(test)]").unwrap_or(src.len());
    let lines: Vec<&str> = src[..cut].lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.trim_start().starts_with("#[serde(default") {
            continue;
        }
        // The field the attribute decorates: the next line that is neither a
        // doc comment nor another attribute.
        let Some((j, field_line)) = lines.iter().enumerate().skip(i + 1).find(|(_, l)| {
            let t = l.trim_start();
            !t.starts_with("///") && !t.starts_with("#[") && !t.starts_with("//")
        }) else {
            continue;
        };
        let t = field_line
            .trim()
            .trim_start_matches("pub ")
            .trim_start_matches("pub(crate) ");
        let Some((field, ty)) = t.split_once(':') else {
            continue;
        };
        let ty = ty.trim().trim_end_matches(',').trim();
        if ty.contains('<') {
            continue; // Option<…>, Vec<…>, Policy<…>: not a bare enum.
        }
        let name = ty.rsplit("::").next().unwrap_or(ty);
        if !enums.contains(name) || policy_enums.contains(name) {
            continue;
        }
        let key = format!("{service}/{}", field.trim());
        if allowed
            .iter()
            .any(|(k, reason)| *k == key && !reason.trim().is_empty())
        {
            continue;
        }
        out.push((
            format!("{file}:{}", j + 1),
            format!(
                "`{}: {ty}` defaults an enum silently — declare it with policy_choice! as a \
                 `Policy<…>` under `[policy]`, or list `{key}` in NOT_POLICY with the reason it \
                 is not an operator reading",
                field.trim()
            ),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy_inventory::parse_source;

    fn decl(src: &str) -> PolicyDecl {
        parse_source("x", "services/x/src/config.rs", src).remove(0)
    }

    /// SC-003: an option without a source is refused.
    #[test]
    fn an_option_without_a_source_is_refused() {
        let d = decl(
            r#"mako_service::policy_choice! {
    pub enum P {
        key: "p",
        doc: "@/docs/x.md#policy-p",
        default: tightening(A, "safe"),
        options: {
            A = "a" => "Quelle A",
            B = "b" => "",
        }
    }
}"#,
        );
        let f = declaration_findings(&d);
        assert!(
            f.iter()
                .any(|(_, m)| m.contains("`b` names no primary source")),
            "{f:?}"
        );
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn a_tightening_default_without_a_reason_is_refused() {
        let d = decl(
            r#"mako_service::policy_choice! {
    pub enum P {
        key: "p",
        doc: "@/docs/x.md#policy-p",
        default: tightening(A, ""),
        options: {
            A = "a" => "Quelle A",
        }
    }
}"#,
        );
        assert!(
            declaration_findings(&d)
                .iter()
                .any(|(_, m)| m.contains("no reason"))
        );
    }

    #[test]
    fn a_known_wrong_source_is_refused() {
        // Assembled at run time, so this file does not itself carry the
        // citation check-citations refuses.
        let repealed = ["Mess", "ZV"].concat();
        let d = decl(&format!(
            r#"mako_service::policy_choice! {{
    pub enum P {{
        key: "p",
        doc: "@/docs/x.md#policy-p",
        default: tightening(A, "safe"),
        options: {{
            A = "a" => "§ 12 {repealed}",
        }}
    }}
}}"#
        ));
        assert!(!declaration_findings(&d).is_empty());
    }

    /// SC-003: a bare defaulted enum in a service config is refused, a policy
    /// field and an allow-listed one are not.
    #[test]
    fn a_bare_defaulted_enum_is_refused_and_a_policy_is_not() {
        let src = "
pub struct C {
    /// A choice nobody sees.
    #[serde(default)]
    pub politik: BeihilfePolitik,
    #[serde(default)]
    pub format: LogFormat,
    #[serde(default)]
    pub beihilfe_politik: mako_service::policy::Policy<BeihilfePolitik>,
    #[serde(default)]
    pub flag: bool,
}
";
        let enums: BTreeSet<String> = ["BeihilfePolitik", "LogFormat"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let none = BTreeSet::new();
        let f = defaulted_enum_findings("services/x/src/config.rs", src, &enums, &none, &[]);
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[0].0.ends_with(":5"), "{f:?}");
        let allowed = [("x/format", "a log format, not a reading of the law")];
        let policies: BTreeSet<String> = ["BeihilfePolitik".to_owned()].into();
        let f =
            defaulted_enum_findings("services/x/src/config.rs", src, &enums, &policies, &allowed);
        assert!(f.is_empty(), "{f:?}");
    }
}
