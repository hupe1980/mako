//! Guard: a configuration check reports findings, it never refuses with an
//! error value.
//!
//! `Daemon::validate` returns every `Befund` it finds — each with the setting's
//! path and, where a document fixes the rule, its source — and the runner
//! derives the boot refusal from that list, serves it at
//! `GET /admin/einrichtung` and prints it from `--check-config`. A `bail!`,
//! `anyhow!` or `ensure!` inside such a check stops at the first defect and
//! carries no path: the operator fixes one problem per restart again, and the
//! readiness view cannot place the finding. It cannot even compile in
//! `validate` itself, which returns no `Result` — but it compiles in a helper
//! that returns one, so the guard reads both:
//!
//! - the body of `fn validate` inside every `impl … Daemon for …`;
//! - the body of every function that returns `Vec<…Befund>` or takes a
//!   `&mut Vec<…Befund>` to collect into, under `services/*/src` and
//!   `crates/mako-service/src`.
//!
//! A helper that legitimately returns an error (an auth posture `build` also
//! calls) is called from `validate` and its error converted with
//! `Befund::aus_fehler(<path>, e)` — which names the path the guard asks for.

use std::path::{Path, PathBuf};

/// The macros that refuse with an error value.
const REFUSALS: [&str; 3] = ["bail!", "anyhow!", "ensure!"];

/// The lowest number of checks a healthy scan finds — below it the walk or
/// the signature match has stopped matching and a clean result is about
/// nothing.
const MIN_CHECKS: usize = 15;

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// `src` with comments, string and char literals blanked to spaces (newlines
/// kept), so braces and macro names inside them neither nest nor match.
fn code_only(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, c: u8| out.push(if c == b'\n' { b'\n' } else { b' ' });
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                blank(&mut out, b[i]);
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let mut depth = 0;
            while i < b.len() {
                if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                } else if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    out.extend_from_slice(b"  ");
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    blank(&mut out, b[i]);
                    i += 1;
                }
            }
        } else if c == b'r'
            && (b.get(i + 1) == Some(&b'"') || b.get(i + 1) == Some(&b'#'))
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
        {
            // Raw string: r"…", r#"…"#.
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&b'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&b'"') {
                out.push(c);
                i += 1;
                continue;
            }
            let close: Vec<u8> = std::iter::once(b'"')
                .chain(std::iter::repeat_n(b'#', hashes))
                .collect();
            let end = src[j + 1..]
                .find(std::str::from_utf8(&close).unwrap_or("\""))
                .map_or(b.len(), |k| j + 1 + k + close.len());
            for &x in &b[i..end] {
                blank(&mut out, x);
            }
            i = end;
        } else if c == b'"' {
            blank(&mut out, c);
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    blank(&mut out, b[i]);
                    i += 1;
                }
                if i < b.len() {
                    blank(&mut out, b[i]);
                    i += 1;
                }
            }
            if i < b.len() {
                blank(&mut out, b[i]);
                i += 1;
            }
        } else if c == b'\'' {
            // A char literal is 'x', '\n', '\u{..}'; anything else is a lifetime.
            let end = if b.get(i + 1) == Some(&b'\\') {
                b[i + 2..]
                    .iter()
                    .position(|&x| x == b'\'')
                    .map(|k| i + 2 + k + 1)
            } else {
                let ch_len = src[i + 1..].chars().next().map_or(1, char::len_utf8);
                (b.get(i + 1 + ch_len) == Some(&b'\'')).then_some(i + 2 + ch_len)
            };
            match end {
                Some(end) => {
                    for &x in &b[i..end] {
                        blank(&mut out, x);
                    }
                    i = end;
                }
                None => {
                    out.push(c);
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `{ … }` body starting at the first `{` at or after `from`, as a byte
/// range, or `None` for a declaration without a body.
fn body(code: &str, from: usize) -> Option<(usize, usize)> {
    let b = code.as_bytes();
    let mut i = from;
    while i < b.len() && b[i] != b'{' {
        if b[i] == b';' {
            return None;
        }
        i += 1;
    }
    let start = i;
    let mut depth = 0usize;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((start, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The checked bodies of `code`: `(name, line, body)`.
fn checks(code: &str) -> Vec<(String, usize, String)> {
    let mut out = Vec::new();
    let line_of = |at: usize| code[..at].matches('\n').count() + 1;
    // `impl … Daemon for …` blocks: their `fn validate`.
    let mut rest = 0;
    while let Some(k) = code[rest..].find("impl") {
        let at = rest + k;
        rest = at + 4;
        let head_end = code[at..].find('{').map_or(code.len(), |e| at + e);
        let head = &code[at..head_end];
        if !(head.contains("Daemon for") || head.contains("Daemon\nfor")) {
            continue;
        }
        let Some((s, e)) = body(code, at) else {
            continue;
        };
        let block = &code[s..e];
        if let Some(v) = block.find("fn validate(")
            && let Some((vs, ve)) = body(block, v)
        {
            out.push((
                "Daemon::validate".to_owned(),
                line_of(s + v),
                block[vs..ve].to_owned(),
            ));
        }
        rest = e;
    }
    // Functions that collect findings.
    let mut rest = 0;
    while let Some(k) = code[rest..].find("fn ") {
        let at = rest + k;
        rest = at + 3;
        if at > 0 && {
            let p = code.as_bytes()[at - 1];
            p.is_ascii_alphanumeric() || p == b'_'
        } {
            continue;
        }
        let sig_end = match (code[at..].find('{'), code[at..].find(';')) {
            (Some(a), Some(b)) => at + a.min(b),
            (Some(a), None) => at + a,
            _ => continue,
        };
        let sig: String = code[at..sig_end].split_whitespace().collect();
        let collects = sig.contains("->Vec<Befund>")
            || sig.contains("->Vec<mako_service::Befund>")
            || sig.contains("->Vec<mako_service::befund::Befund>")
            || sig.contains("->Vec<crate::befund::Befund>")
            || sig.contains("&mutVec<Befund>")
            || sig.contains("&mutVec<mako_service::befund::Befund>")
            || sig.contains("&mutVec<mako_service::Befund>")
            || sig.contains("&mutVec<crate::befund::Befund>");
        if !collects {
            continue;
        }
        let name: String = sig[2..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if let Some((s, e)) = body(code, at) {
            out.push((name, line_of(at), code[s..e].to_owned()));
        }
    }
    out
}

/// Refusals inside the checked bodies of one file.
fn findings(rel: &str, src: &str) -> (Vec<String>, usize) {
    let code = code_only(src);
    let found = checks(&code);
    let mut out = Vec::new();
    for (name, line, body) in &found {
        for m in REFUSALS {
            if body.contains(m) {
                out.push(format!(
                    "{rel}:{line}: `{name}` refuses with `{m}` — a configuration check returns \
                     every finding as a `Befund` with its path (`Befund::blockiert(pfad, text)`, \
                     or `Befund::aus_fehler(pfad, e)` for a helper's error), never the first \
                     as an error"
                ));
            }
        }
    }
    (out, found.len())
}

pub fn run(root: &Path) -> bool {
    let mut files = Vec::new();
    rust_files(&root.join("crates/mako-service/src"), &mut files);
    let mut services: Vec<PathBuf> = std::fs::read_dir(root.join("services"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("src"))
        .collect();
    services.sort();
    for s in services {
        rust_files(&s, &mut files);
    }
    let mut out = Vec::new();
    let mut checked = 0;
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        let rel = f.strip_prefix(root).unwrap_or(f).display().to_string();
        let (o, n) = findings(&rel, &src);
        out.extend(o);
        checked += n;
    }
    if checked < MIN_CHECKS {
        eprintln!(
            "check-config-findings: found {checked} configuration checks, at least {MIN_CHECKS} \
             expected — the scan has stopped matching"
        );
        return false;
    }
    if out.is_empty() {
        println!(
            "check-config-findings: {checked} configuration checks in {} files report findings, \
             none refuses with an error value",
            files.len()
        );
        return true;
    }
    for f in &out {
        eprintln!("check-config-findings: {f}");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAEMON: &str = r#"
impl mako_service::Daemon for X {
    type Config = C;
    fn validate(cfg: &C) -> Vec<Befund> {
        let mut out = Vec::new();
        if cfg.a { out.push(Befund::blockiert("a", "format {x} with } brace")); }
        out
    }
}
"#;

    #[test]
    fn a_daemon_validate_collecting_findings_passes() {
        let (out, n) = findings("x.rs", DAEMON);
        assert_eq!(n, 2, "the impl's validate and the Vec<Befund> fn");
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_bail_in_a_daemon_validate_is_refused() {
        let src = DAEMON.replace("out\n    }", "anyhow::bail!(\"x\");\n    }");
        let (out, _) = findings("x.rs", &src);
        assert!(out.iter().any(|o| o.contains("`bail!`")), "{out:?}");
    }

    #[test]
    fn a_collecting_helper_that_refuses_is_refused() {
        let src = "fn rows(r: &[u8], out: &mut Vec<Befund>) {\n    anyhow::ensure!(r.is_empty(), \"x\");\n}\n\
                   fn other() -> Vec<mako_service::befund::Befund> { vec![anyhow::anyhow!(\"y\")] }\n";
        let (out, n) = findings("x.rs", src);
        assert_eq!(n, 2);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(out[0].contains("x.rs:1: `rows`"), "{out:?}");
    }

    #[test]
    fn a_refusal_outside_a_check_or_in_a_string_is_not_its_business() {
        let src = "fn build() -> anyhow::Result<()> { anyhow::bail!(\"x\") }\n\
                   fn check() -> Vec<Befund> { vec![Befund::blockiert(\"a\", \"never bail!(\")] }\n\
                   // fn validate() -> Vec<Befund> { bail!() }\n";
        let (out, n) = findings("x.rs", src);
        assert_eq!(n, 1);
        assert!(out.is_empty(), "{out:?}");
    }

    /// The real tree: the scan finds the daemons' checks and none refuses.
    #[test]
    fn the_real_tree_passes_and_is_seen() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        assert!(run(root));
    }
}
