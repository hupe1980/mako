//! `cargo xtask policy-inventory` — every operator policy setting, read from
//! the `mako_service::policy_choice!` declarations, as one generated docs page
//! and (with `--json`) the release evidence pack's `policy_defaults` input.
//!
//! ```text
//! cargo xtask policy-inventory            # write site/content/docs/reference/operator-policy.md
//! cargo xtask policy-inventory --check    # refuse a page that differs from the declarations
//! cargo xtask policy-inventory --json <f> # write the evidence-pack input
//! ```
//!
//! The declarations are the one source: the page is never edited by hand, and
//! `check-policy-settings` refuses it when it has drifted.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// The generated page, relative to the workspace root.
pub const PAGE: &str = "site/content/docs/reference/operator-policy.md";

/// One option of a setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyOption {
    /// The enum variant.
    pub variant: String,
    /// The name as written in the config.
    pub name: String,
    /// Its primary source.
    pub quelle: String,
}

/// The default of a setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDefault {
    /// The tightening option's variant and the reason.
    Tightening { variant: String, reason: String },
    /// No default.
    Required,
}

/// One `policy_choice!` declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecl {
    /// The service directory under `services/` (or the crate path).
    pub service: String,
    /// The declaring file, workspace-relative.
    pub file: String,
    /// The 1-based line of the `enum`.
    pub line: usize,
    /// The enum's name.
    pub enum_name: String,
    /// The first sentence of its doc comment.
    pub summary: String,
    /// `[policy]` key.
    pub key: String,
    /// Docs anchor (`@/docs/….md#anchor`).
    pub doc: String,
    /// Default.
    pub default: PolicyDefault,
    /// `(date, grund)`.
    pub review_by: Option<(String, String)>,
    /// Options in declaration order.
    pub options: Vec<PolicyOption>,
}

impl PolicyDecl {
    /// The default option's config name, or `required`.
    #[must_use]
    pub fn default_name(&self) -> String {
        match &self.default {
            PolicyDefault::Required => "required".to_owned(),
            PolicyDefault::Tightening { variant, .. } => self
                .options
                .iter()
                .find(|o| &o.variant == variant)
                .map_or_else(|| format!("?{variant}"), |o| o.name.clone()),
        }
    }
}

/// The first string literal in `s`, with `\"` unescaped.
fn literal(s: &str) -> Option<(String, &str)> {
    let start = s.find('"')?;
    let mut out = String::new();
    let mut chars = s[start + 1..].char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                if let Some((_, n)) = chars.next() {
                    out.push(n);
                }
            }
            '"' => return Some((out, &s[start + 1 + i + 1..])),
            _ => out.push(c),
        }
    }
    None
}

/// Every `policy_choice!` declaration in one source file.
#[must_use]
pub fn parse_source(service: &str, file: &str, src: &str) -> Vec<PolicyDecl> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i]
            .trim_start()
            .starts_with("mako_service::policy_choice! {")
            && !lines[i].trim_start().starts_with("crate::policy_choice! {")
        {
            i += 1;
            continue;
        }
        let mut doc = Vec::new();
        let mut decl = PolicyDecl {
            service: service.to_owned(),
            file: file.to_owned(),
            line: i + 1,
            enum_name: String::new(),
            summary: String::new(),
            key: String::new(),
            doc: String::new(),
            default: PolicyDefault::Required,
            review_by: None,
            options: Vec::new(),
        };
        let mut in_options = false;
        let mut seen_enum = false;
        i += 1;
        while i < lines.len() {
            let t = lines[i].trim();
            if !seen_enum {
                if let Some(d) = t.strip_prefix("///") {
                    doc.push(d.trim().to_owned());
                } else if let Some(pos) = t.find("enum ") {
                    let name: String = t[pos + 5..]
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    decl.enum_name = name;
                    decl.line = i + 1;
                    seen_enum = true;
                }
            } else if in_options {
                if t.starts_with('}') {
                    in_options = false;
                } else if !t.starts_with("///") && !t.starts_with("#[") && t.contains("=>") {
                    let (lhs, rhs) = t.split_once("=>").unwrap_or((t, ""));
                    let variant = lhs.split('=').next().unwrap_or("").trim().to_owned();
                    let name = literal(lhs).map(|(n, _)| n).unwrap_or_default();
                    let quelle = literal(rhs).map(|(q, _)| q).unwrap_or_default();
                    decl.options.push(PolicyOption {
                        variant,
                        name,
                        quelle,
                    });
                }
            } else if let Some(r) = t.strip_prefix("key:") {
                decl.key = literal(r).map(|(k, _)| k).unwrap_or_default();
            } else if let Some(r) = t.strip_prefix("doc:") {
                decl.doc = literal(r).map(|(d, _)| d).unwrap_or_default();
            } else if let Some(r) = t.strip_prefix("default:") {
                let r = r.trim();
                if let Some(rest) = r.strip_prefix("tightening(") {
                    let variant: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    let reason = literal(rest).map(|(x, _)| x).unwrap_or_default();
                    decl.default = PolicyDefault::Tightening { variant, reason };
                } else {
                    decl.default = PolicyDefault::Required;
                }
            } else if let Some(r) = t.strip_prefix("review_by:") {
                let nums: Vec<String> = r
                    .split([',', '(', ')'])
                    .map(str::trim)
                    .filter(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit()))
                    .map(str::to_owned)
                    .take(3)
                    .collect();
                let grund = literal(r).map(|(g, _)| g).unwrap_or_default();
                if let [y, m, d] = nums.as_slice() {
                    decl.review_by = Some((format!("{y}-{m:0>2}-{d:0>2}"), grund));
                }
            } else if t.starts_with("options:") {
                in_options = true;
            } else if t == "}" || t.starts_with("} ") {
                // End of the enum body.
                i += 1;
                break;
            }
            i += 1;
        }
        decl.summary = summary(&doc);
        out.push(decl);
    }
    out
}

/// The first sentence of a doc comment.
fn summary(doc: &[String]) -> String {
    let joined = doc
        .iter()
        .take_while(|l| !l.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    match joined.find(". ") {
        Some(i) => joined[..=i].to_owned(),
        None => joined,
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
            if p.file_name().is_some_and(|n| n == "target" || n == "tests") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Every declaration under `services/*/src`, ordered by service, then key.
#[must_use]
pub fn collect(root: &Path) -> Vec<PolicyDecl> {
    let mut out = Vec::new();
    let Ok(services) = std::fs::read_dir(root.join("services")) else {
        return out;
    };
    let mut services: Vec<_> = services.flatten().map(|e| e.path()).collect();
    services.sort();
    for svc in services {
        let name = svc
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut files = Vec::new();
        rust_files(&svc.join("src"), &mut files);
        for f in files {
            let Ok(src) = std::fs::read_to_string(&f) else {
                continue;
            };
            let rel = f
                .strip_prefix(root)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            out.extend(parse_source(&name, &rel, &src));
        }
    }
    out.sort_by(|a, b| (&a.service, &a.key).cmp(&(&b.service, &b.key)));
    out
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

/// The page.
#[must_use]
pub fn render(decls: &[PolicyDecl]) -> String {
    let mut s = String::from(
        "+++
title = \"Operator policy\"
description = \"Every operator setting mako ships: its key, its options with their primary sources, and its default or that it is required.\"
weight = 19
+++
<!-- Generated by `cargo xtask policy-inventory` from the `policy_choice!`
     declarations. Do not edit: `check-policy-settings` refuses a page that
     differs from them. -->

A setting exists where a primary source leaves the answer to the operator —
an unsettled reading of the law, or a choice two lawful options offer. Where a
source settles the question there is no setting, because a setting would be a
way to configure a breach.

Each service reads its settings from its own `[policy]` section. A setting
with a default takes the **tightening** option when it is left out: the one
that cannot cause a breach or an overpayment. A **required** setting has no
default, and the service refuses to start until it is set. Every service logs
each setting at startup with its origin (`explicit` or `default`) and source,
exports it as `mako_policy_info{service,key,value,origin,class}` on
`/metrics`, records each change in `service_policy_log`, and serves the report
read-only at `GET /api/v1/policy` and through the MCP tool `get_policy`. A
setting changes only by editing the config and restarting.

| Service | Key | Default | Options and sources | Documented at |
|---|---|---|---|---|
",
    );
    for d in decls {
        let options = d
            .options
            .iter()
            .map(|o| format!("`{}` — {}", o.name, cell(&o.quelle)))
            .collect::<Vec<_>>()
            .join("<br>");
        let default = match &d.default {
            PolicyDefault::Required => "**required**".to_owned(),
            PolicyDefault::Tightening { reason, .. } => {
                format!("`{}` — {}", d.default_name(), cell(reason))
            }
        };
        s.push_str(&format!(
            "| `{}` | `{}` | {} | {} | [{}]({}) |\n",
            d.service, d.key, default, options, d.service, d.doc
        ));
    }
    let reviews: Vec<&PolicyDecl> = decls.iter().filter(|d| d.review_by.is_some()).collect();
    if !reviews.is_empty() {
        s.push_str(
            "\n## Review dates\n\nA setting that represents law not yet enacted carries a review date. After it, \
             the service warns at every startup; the value never changes by itself.\n\n\
             | Service | Key | Review by | What is reviewed |\n|---|---|---|---|\n",
        );
        for d in reviews {
            if let Some((date, grund)) = &d.review_by {
                s.push_str(&format!(
                    "| `{}` | `{}` | {} | {} |\n",
                    d.service,
                    d.key,
                    date,
                    cell(grund)
                ));
            }
        }
    }
    s
}

/// The evidence-pack input: every setting with its default and the source of
/// each option.
#[must_use]
pub fn to_json(decls: &[PolicyDecl]) -> Value {
    Value::Array(
        decls
            .iter()
            .map(|d| {
                json!({
                    "service": d.service,
                    "key": d.key,
                    "default": d.default_name(),
                    "review_by": d.review_by.as_ref().map(|(date, _)| date),
                    "options": d.options.iter().map(|o| json!({
                        "name": o.name,
                        "quelle": o.quelle,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

pub fn run(root: &Path, args: &[String]) -> bool {
    if let Some(bad) = unknown_argument(args) {
        eprintln!(
            "policy-inventory: unknown argument `{bad}` — usage: cargo xtask policy-inventory [--check | --json <file>]"
        );
        return false;
    }
    let decls = collect(root);
    if decls.is_empty() {
        eprintln!(
            "policy-inventory: found no policy_choice! declaration — has the layout changed?"
        );
        return false;
    }
    if let Some(i) = args.iter().position(|a| a == "--json") {
        let Some(out) = args.get(i + 1) else {
            eprintln!("usage: cargo xtask policy-inventory --json <file>");
            return false;
        };
        let text = serde_json::to_string_pretty(&to_json(&decls)).expect("a JSON value serialises");
        if let Err(e) = std::fs::write(out, text + "\n") {
            eprintln!("policy-inventory: cannot write {out}: {e}");
            return false;
        }
        println!("policy-inventory: wrote {} settings to {out}", decls.len());
        return true;
    }
    let page = render(&decls);
    let path = root.join(PAGE);
    if args.iter().any(|a| a == "--check") {
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current == page {
            println!("policy-inventory: {PAGE} matches {} settings", decls.len());
            return true;
        }
        eprintln!(
            "policy-inventory: {PAGE} differs from the declarations — run `cargo xtask policy-inventory`"
        );
        return false;
    }
    if let Err(e) = std::fs::write(&path, page) {
        eprintln!("policy-inventory: cannot write {PAGE}: {e}");
        return false;
    }
    println!("policy-inventory: wrote {PAGE} ({} settings)", decls.len());
    true
}

/// The first argument that is neither `--check` nor `--json <file>`: an
/// unrecognised flag must refuse, not fall through to writing the page.
fn unknown_argument(args: &[String]) -> Option<&str> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--check" => {}
            "--json" => {
                it.next();
            }
            other => return Some(other),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--help` (or any unknown flag) refuses and writes nothing: running the
    /// tool to read its usage must not regenerate the tracked page.
    #[test]
    fn an_unknown_argument_is_refused_and_writes_nothing() {
        let args = |v: &[&str]| v.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
        assert_eq!(unknown_argument(&args(&["--help"])), Some("--help"));
        assert_eq!(unknown_argument(&args(&["--check", "-v"])), Some("-v"));
        assert_eq!(unknown_argument(&args(&["--check"])), None);
        assert_eq!(unknown_argument(&args(&["--json", "out.json"])), None);
        assert_eq!(unknown_argument(&[]), None);
        let dir = std::env::temp_dir().join("mako-policy-inventory-unknown-arg");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!run(&dir, &args(&["--help"])));
        assert!(
            !dir.join(PAGE).exists(),
            "a refused run must not write the page"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    const SRC: &str = r#"
mako_service::policy_choice! {
    /// What happens. More text.
    pub enum Politik {
        key: "politik",
        doc: "@/docs/services/x.md#policy-politik",
        default: tightening(Nein, "cannot be taken back"),
        review_by: (2027, 1, 1, "the law is enacted"),
        options: {
            /// No.
            Nein = "nein" => "Quelle A, S. \"1\"",
            Ja = "ja" => "Quelle B",
        }
    }
}
"#;

    #[test]
    fn a_declaration_is_read_with_every_part() {
        let d = parse_source("x", "services/x/src/config.rs", SRC);
        assert_eq!(d.len(), 1);
        let d = &d[0];
        assert_eq!(d.enum_name, "Politik");
        assert_eq!(d.key, "politik");
        assert_eq!(d.summary, "What happens.");
        assert_eq!(d.default_name(), "nein");
        assert_eq!(
            d.review_by,
            Some(("2027-01-01".to_owned(), "the law is enacted".to_owned()))
        );
        assert_eq!(d.options.len(), 2);
        assert_eq!(d.options[0].quelle, "Quelle A, S. \"1\"");
        let page = render(std::slice::from_ref(d));
        assert!(
            page.contains("| `x` | `politik` | `nein` — cannot be taken back |"),
            "{page}"
        );
    }
}
