//! Guard: `Workflow::handle` and `Workflow::apply` are pure.
//!
//! `concepts/PLATFORM.md` §2 and §10 and `AGENTS.md` („Workflow determinism")
//! state the contract: a workflow is a pure function of `(state, command)` —
//! no I/O, no clock access, no global state mutation. The engine relies on it
//! twice over. `Process::execute_with_retry` re-runs `handle` after a
//! `VersionConflict`, so an impure handler can emit a *different message* on the
//! second attempt than the first attempt put on the wire; and every state is
//! reconstructed by folding the event log, so a replay of an impure workflow
//! produces bytes that were never sent.
//!
//! The shape refused here is a handler reading the wall clock, and two failures
//! follow from it — both silent. A Frist derived from `now_utc()` is anchored on
//! the instant the handler happens to run rather than on the message's
//! Übertragungszeitpunkt, so a regulatory window moves with scheduling. And a
//! clock-derived value that reaches an outbound payload makes a retry render
//! different bytes than the first attempt put on the wire: the `DTM+273`
//! Bindungsfrist of a QUOTES 15003 is a **duration**, so it is exactly that
//! shape. The rule in both cases is the same and is the repo's established
//! idiom: the command carries the instant (`received_at` for an
//! inbound message's arrival, `gesendet_am` for an outbound message's ÜT), so
//! the value is resolved once, at the makod boundary, where reading the clock is
//! exactly the right thing to do.
//!
//! ## What is scanned
//!
//! Every `.rs` file under `crates/*/src/**` and `services/*/src/**`. Within each
//! file, only the bodies of `impl … Workflow for …` blocks — brace-matched, so
//! the whole trait impl is covered rather than a heuristic window around `fn
//! handle`. `apply`, `on_deadline` and any private helper written inside the
//! impl are held to the same rule, because all three run on the replay path.
//!
//! `#[cfg(test)]` sections are blanked out first: a test workflow reading the
//! clock to build a fixture is not a production defect. Comments and string
//! literals are skipped too, so a doc comment *explaining* why the handler does
//! not call `now_utc()` — the comments the mako-wim fix added — does not fail
//! the check it documents.
//!
//! A helper called *from* a handler but written outside the impl block is out of
//! reach of this guard. It matches what the rule can be enforced on textually;
//! it is not a claim that the call graph is pure.
//!
//! ## Why it refuses an empty tree
//!
//! A scanner that silently stops matching certifies nothing — it reports the
//! clean line for a tree it never read. [`MIN_WORKFLOW_IMPLS`] is the floor: the
//! workspace holds far more than that many production workflow impls, so a run
//! that finds fewer means the `impl … Workflow for …` shape moved and the guard
//! has to be taught the new one, not that the defect class went away.

use std::path::Path;

/// Trees holding the workflow implementations.
const SCANNED: &[&str] = &["crates", "services"];

/// The lowest number of production `impl … Workflow for …` blocks a healthy
/// scan may find.
///
/// There were 62 when this guard was written. The floor sits far enough below
/// that a genuine deletion of a workflow or two does not trip it, and far enough
/// above zero that a scanner which stopped matching cannot pass.
const MIN_WORKFLOW_IMPLS: usize = 40;

/// What a pure workflow may not reach for, and what the call actually is.
///
/// Every entry is a value the workflow has to *receive* rather than obtain: the
/// engine re-runs `handle` on a version conflict and re-folds the log on every
/// read, so any of these makes the same (state, command) pair produce two
/// different answers.
const FORBIDDEN: &[(&str, &str)] = &[
    ("now_utc()", "reads the wall clock"),
    ("SystemTime::now", "reads the wall clock"),
    ("Instant::now", "reads the monotonic clock"),
    ("Uuid::new_v4", "draws a fresh random identifier"),
    ("thread_rng", "draws randomness"),
    ("rand::random", "draws randomness"),
    ("std::env::var", "reads the process environment"),
    ("std::fs::", "touches the filesystem"),
];

/// A violating call site: workspace-relative path, 1-based line, the forbidden
/// token, and the source line it sits on.
type Finding = (String, usize, &'static str, String);

/// Run the check, returning `true` when every workflow impl is pure.
pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    for tree in SCANNED {
        collect_sources(&workspace_root.join(tree), &mut files);
    }

    let mut impls = 0usize;
    let mut findings: Vec<Finding> = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let (found, mut hits) = scan_source(&src, &rel);
        impls += found;
        findings.append(&mut hits);
    }

    // A guard that matched nothing finds no impurity, and prints the clean line
    // for it. Refuse the tree instead.
    if impls < MIN_WORKFLOW_IMPLS {
        eprintln!(
            "check-workflow-purity: found only {impls} `impl … Workflow for …` block(s) in \
             {} source file(s) under crates/*/src and services/*/src — at least \
             {MIN_WORKFLOW_IMPLS} are expected.\n\
             The scanner has stopped matching the shape it is looking for. Fix the scanner \
             before trusting a green run; a check that recognises nothing passes everything.",
            files.len(),
        );
        return false;
    }

    if findings.is_empty() {
        println!(
            "check-workflow-purity: all {impls} `impl … Workflow for …` block(s) are pure \
             (no clock, randomness, environment or filesystem access)"
        );
        return true;
    }

    eprintln!(
        "check-workflow-purity: {} site(s) inside a `Workflow` impl reach outside the \
         (state, command) pair:",
        findings.len()
    );
    for (path, line, token, text) in &findings {
        let reason = FORBIDDEN
            .iter()
            .find(|(t, _)| t == token)
            .map_or("is not a function of the inputs", |(_, r)| *r);
        eprintln!("  {path}:{line}  `{token}` — {reason}");
        eprintln!("      {}", text.trim());
    }
    eprintln!(
        "\n`Workflow::handle` and `Workflow::apply` are pure functions of (state, command): \
         no I/O, no clock access, no global state mutation (concepts/PLATFORM.md §2/§10, \
         AGENTS.md „Workflow determinism\").\n\
         `Process::execute_with_retry` re-runs `handle` after a version conflict and every \
         state is re-folded from the event log, so a value taken from the environment here \
         makes a retry emit a different message than the one already sent, and a replay \
         render bytes that never went on the wire.\n\
         The value must arrive as a **command field**, resolved at the makod boundary: \
         `received_at` for the instant an inbound message arrived, `gesendet_am` for the \
         Übertragungszeitpunkt of an outbound one. See \
         `crates/mako-gpke/src/neuanlage.rs` for the pattern."
    );
    false
}

/// Every `.rs` file under `<tree>/*/src/**`.
fn collect_sources(tree: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(tree) else {
        return;
    };
    for entry in entries.flatten() {
        let src_dir = entry.path().join("src");
        if src_dir.is_dir() {
            walk_rs(&src_dir, out);
        }
    }
}

/// Recursively collect `.rs` files under `dir`.
fn walk_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// Scan one source file. Returns how many `impl … Workflow for …` blocks it
/// held and every violation inside them.
fn scan_source(src: &str, rel: &str) -> (usize, Vec<Finding>) {
    // Comments and string literals are not code; a `#[cfg(test)]` section is not
    // production code. Both are removed from the mask so neither can match a
    // forbidden token nor confuse the brace matching.
    let mut mask = code_mask(src);
    blank_cfg_test(src, &mut mask);

    let bytes = src.as_bytes();
    let mut impls = 0usize;
    let mut findings = Vec::new();

    for (start, _) in src.match_indices("impl") {
        if !mask[start] || !is_word_start(bytes, start) || !is_word_end(bytes, start + 4) {
            continue;
        }
        // Header runs from `impl` to the brace that opens the impl body.
        let Some(open) = next_code_byte(bytes, &mask, start, b'{') else {
            continue;
        };
        if !implements_workflow(&src[start + 4..open]) {
            continue;
        }
        let Some(close) = match_brace(bytes, &mask, open) else {
            continue;
        };
        impls += 1;
        let body = &src[open..close];
        for (token, _) in FORBIDDEN {
            for (off, _) in body.match_indices(token) {
                let abs = open + off;
                if !mask[abs] {
                    continue;
                }
                findings.push((
                    rel.to_owned(),
                    line_of(src, abs),
                    *token,
                    line_text(src, abs).to_owned(),
                ));
            }
        }
    }
    (impls, findings)
}

/// Whether an impl header's trait position names `Workflow`.
///
/// `header` is everything between `impl` and the body's `{`. Generic parameters
/// are skipped first, so `impl<W: Workflow, S> Clone for Process<W, S>` — where
/// `Workflow` is only a bound — is correctly *not* a workflow impl, while
/// `impl<F: InvoicFamily> Workflow for InvoicWorkflow<F>` is. A fully qualified
/// `mako_engine::workflow::Workflow` counts by its last path segment.
fn implements_workflow(header: &str) -> bool {
    let rest = skip_generics(header.trim_start());
    let Some(trait_part) = split_on_for(rest) else {
        return false; // inherent impl — no trait position at all
    };
    let trait_path = trait_part.trim().trim_start_matches('<').trim();
    let base = trait_path
        .split('<')
        .next()
        .unwrap_or(trait_path)
        .trim()
        .trim_end_matches('!');
    base.rsplit("::").next().unwrap_or(base).trim() == "Workflow"
}

/// Drop a leading `<…>` generic parameter list, angle-bracket balanced.
fn skip_generics(s: &str) -> &str {
    if !s.starts_with('<') {
        return s;
    }
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    return s[i + 1..].trim_start();
                }
            }
            _ => {}
        }
    }
    s
}

/// The text before the `for` keyword that separates trait from type, at angle
/// depth 0. `None` for an inherent impl.
fn split_on_for(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b'f' if depth == 0
                && s[i..].starts_with("for")
                && is_word_start(bytes, i)
                && is_word_end(bytes, i + 3) =>
            {
                return Some(&s[..i]);
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The next occurrence of `needle` at or after `from` that is real code.
fn next_code_byte(bytes: &[u8], mask: &[bool], from: usize, needle: u8) -> Option<usize> {
    (from..bytes.len()).find(|&i| bytes[i] == needle && mask[i])
}

/// The `}` closing the `{` at `open`, counting only unmasked braces.
fn match_brace(bytes: &[u8], mask: &[bool], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for i in open..bytes.len() {
        if !mask[i] {
            continue;
        }
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Blank every `#[cfg(test)]` item out of `mask`.
///
/// A test workflow may read the clock to build a fixture; that is not the defect
/// this guard is about. An attribute on a `use` or a non-braced item stops at
/// its `;` so the following item is not swallowed with it.
fn blank_cfg_test(src: &str, mask: &mut [bool]) {
    let bytes = src.as_bytes();
    for marker in ["#[cfg(test)]", "#![cfg(test)]"] {
        for (start, _) in src.match_indices(marker) {
            if !mask[start] {
                continue;
            }
            let open = next_code_byte(bytes, mask, start, b'{');
            let semi = next_code_byte(bytes, mask, start + marker.len(), b';');
            let end = match (open, semi) {
                // A braced item: blank the whole body.
                (Some(o), s) if s.is_none_or(|s| s > o) => match_brace(bytes, mask, o),
                // `#[cfg(test)] use …;` and friends end at the semicolon.
                (_, Some(s)) => Some(s),
                _ => None,
            };
            let end = end.unwrap_or(bytes.len() - 1);
            for m in &mut mask[start..=end] {
                *m = false;
            }
        }
    }
}

/// A per-byte mask of what is real code: comments, string literals and char
/// literals are `false`.
fn code_mask(src: &str) -> Vec<bool> {
    let b = src.as_bytes();
    let mut mask = vec![true; b.len()];
    let mut i = 0usize;
    while i < b.len() {
        // Line comment.
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                mask[i] = false;
                i += 1;
            }
            continue;
        }
        // Block comment (nesting, as Rust allows).
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let mut depth = 0usize;
            while i < b.len() {
                if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
                    depth += 1;
                    mask[i] = false;
                    mask[i + 1] = false;
                    i += 2;
                    continue;
                }
                if b[i] == b'*' && i + 1 < b.len() && b[i + 1] == b'/' {
                    depth -= 1;
                    mask[i] = false;
                    mask[i + 1] = false;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                mask[i] = false;
                i += 1;
            }
            continue;
        }
        // Raw string: r"…", r#"…"#, br#"…"#.
        if (b[i] == b'r' || b[i] == b'b') && !is_ident_byte_before(b, i) {
            if let Some(next) = raw_string_end(b, i, &mut mask) {
                i = next;
                continue;
            }
        }
        // Ordinary string literal.
        if b[i] == b'"' {
            mask[i] = false;
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    mask[i] = false;
                    if i + 1 < b.len() {
                        mask[i + 1] = false;
                    }
                    i += 2;
                    continue;
                }
                let done = b[i] == b'"';
                mask[i] = false;
                i += 1;
                if done {
                    break;
                }
            }
            continue;
        }
        // Char literal — distinguished from a lifetime by its closing quote.
        if b[i] == b'\'' && is_char_literal(b, i) {
            mask[i] = false;
            i += 1;
            while i < b.len() {
                if b[i] == b'\\' {
                    mask[i] = false;
                    if i + 1 < b.len() {
                        mask[i + 1] = false;
                    }
                    i += 2;
                    continue;
                }
                let done = b[i] == b'\'';
                mask[i] = false;
                i += 1;
                if done {
                    break;
                }
            }
            continue;
        }
        i += 1;
    }
    mask
}

/// Blank a raw string starting at `i` (`r`/`b` prefix). Returns the byte after
/// it, or `None` when this is not a raw string at all.
fn raw_string_end(b: &[u8], i: usize, mask: &mut [bool]) -> Option<usize> {
    let mut j = i;
    if b[j] == b'b' {
        j += 1;
    }
    if j >= b.len() || b[j] != b'r' {
        return None;
    }
    j += 1;
    let hash_start = j;
    while j < b.len() && b[j] == b'#' {
        j += 1;
    }
    let hashes = j - hash_start;
    if j >= b.len() || b[j] != b'"' {
        return None;
    }
    j += 1;
    let terminator: Vec<u8> = std::iter::once(b'"')
        .chain(std::iter::repeat_n(b'#', hashes))
        .collect();
    let end = (j..b.len())
        .find(|&k| b[k..].starts_with(&terminator))
        .map_or(b.len(), |k| k + terminator.len());
    for m in &mut mask[i..end] {
        *m = false;
    }
    Some(end)
}

/// Whether the byte before `i` can continue an identifier (so an `r` there is
/// part of a word, not a raw-string prefix).
fn is_ident_byte_before(b: &[u8], i: usize) -> bool {
    i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')
}

/// Whether the `'` at `i` opens a char literal rather than a lifetime.
fn is_char_literal(b: &[u8], i: usize) -> bool {
    if i + 1 >= b.len() {
        return false;
    }
    if b[i + 1] == b'\\' {
        return true;
    }
    // `'x'` — a single byte then the closing quote. Multi-byte chars are rare
    // enough in literals here that treating them as lifetimes only costs a
    // false negative inside a string-like span, never a false positive.
    i + 2 < b.len() && b[i + 2] == b'\''
}

/// Whether `i` starts a word (no identifier byte immediately before).
fn is_word_start(b: &[u8], i: usize) -> bool {
    !is_ident_byte_before(b, i)
}

/// Whether `i` ends a word (no identifier byte at `i`).
fn is_word_end(b: &[u8], i: usize) -> bool {
    i >= b.len() || !(b[i].is_ascii_alphanumeric() || b[i] == b'_')
}

/// 1-based line number of byte offset `at`.
fn line_of(src: &str, at: usize) -> usize {
    src[..at].bytes().filter(|&c| c == b'\n').count() + 1
}

/// The whole source line byte offset `at` sits on.
fn line_text(src: &str, at: usize) -> &str {
    let start = src[..at].rfind('\n').map_or(0, |i| i + 1);
    let end = src[at..].find('\n').map_or(src.len(), |i| at + i);
    &src[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guard that cannot fail certifies nothing: the scanner has to catch a
    /// workflow that reads the clock inside `handle`.
    #[test]
    fn catches_a_clock_read_inside_a_workflow_impl() {
        let src = "\
impl Workflow for NaughtyWorkflow {
    fn handle(state: &Self::State, cmd: Self::Command) -> Out {
        let now = OffsetDateTime::now_utc();
        decide(state, cmd, now)
    }
}
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 1, "the impl block must be recognised");
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].1, 3, "line number");
        assert_eq!(findings[0].2, "now_utc()");
    }

    /// Every forbidden token is actually matched — an entry nothing can trip is
    /// decoration.
    #[test]
    fn catches_every_forbidden_token() {
        for (token, _) in FORBIDDEN {
            let src = format!("impl Workflow for W {{\n    fn handle() {{ {token} }}\n}}\n");
            let (impls, findings) = scan_source(&src, "crates/x/src/lib.rs");
            assert_eq!(impls, 1, "{token}");
            assert_eq!(findings.len(), 1, "{token} was not caught");
        }
    }

    /// The pure form of the same workflow passes.
    #[test]
    fn accepts_a_pure_workflow_impl() {
        let src = "\
impl Workflow for GoodWorkflow {
    fn handle(state: &Self::State, cmd: Self::Command) -> Out {
        let C::Send { gesendet_am } = cmd;
        decide(state, gesendet_am)
    }
}
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 1);
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    /// A comment saying why the handler does *not* read the clock must not fail
    /// the check it documents — nor may a string literal naming the call.
    #[test]
    fn ignores_comments_and_string_literals() {
        let src = "\
impl Workflow for GoodWorkflow {
    // The instant arrives on the command; calling OffsetDateTime::now_utc()
    // here would make a retry emit a different message.
    fn handle(state: &Self::State, cmd: Self::Command) -> Out {
        Err(reject(\"do not call now_utc() in a workflow\"))
    }
}
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 1);
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    /// A test-only workflow building a fixture off the clock is not the defect.
    #[test]
    fn skips_cfg_test_sections() {
        let src = "\
#[cfg(test)]
mod tests {
    impl Workflow for FixtureWorkflow {
        fn handle() -> Out { OffsetDateTime::now_utc() }
    }
}
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 0, "a #[cfg(test)] impl is not production code");
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    /// `Workflow` in a generic bound is not `Workflow` in the trait position.
    #[test]
    fn a_workflow_bound_is_not_a_workflow_impl() {
        let src = "\
impl<W: Workflow, S: EventStore> Clone for Process<W, S> {
    fn clone(&self) -> Self { Self::new(OffsetDateTime::now_utc()) }
}
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 0);
        assert!(findings.is_empty(), "got: {findings:?}");
    }

    /// …but a generic workflow impl is one.
    #[test]
    fn a_generic_workflow_impl_is_scanned() {
        let src = "\
impl<F: InvoicFamily> Workflow for InvoicWorkflow<F> {
    fn handle() -> Out { OffsetDateTime::now_utc() }
}
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 1);
        assert_eq!(findings.len(), 1);
    }

    /// A fully qualified trait path still names `Workflow`.
    #[test]
    fn a_qualified_workflow_path_is_scanned() {
        let src = "\
impl mako_engine::workflow::Workflow for W {
    fn apply(state: Self::State, e: &Self::Event) -> Self::State { Uuid::new_v4() }
}
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 1);
        assert_eq!(findings.len(), 1);
    }

    /// The impl body is brace-matched: a violation after a nested block still
    /// counts, and one after the impl closes does not.
    #[test]
    fn the_impl_body_is_brace_matched() {
        let src = "\
impl Workflow for W {
    fn handle() -> Out {
        if x { y() }
        Instant::now()
    }
}
fn elsewhere() { let _ = std::time::Instant::now(); }
";
        let (impls, findings) = scan_source(src, "crates/x/src/lib.rs");
        assert_eq!(impls, 1);
        assert_eq!(findings.len(), 1, "got: {findings:?}");
        assert_eq!(findings[0].1, 4);
    }
}
