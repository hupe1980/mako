//! Guard: the Desk's npm tree and its source stay inside two lines.
//!
//! **Licences.** `deny.toml`'s `[licenses] allow` list is the workspace's one
//! allow-list, and the Desk's npm packages are held to it rather than to a
//! second copy. `desk/package-lock.json` (lockfile v3) records each installed
//! package's `license`, so no `node_modules` is needed: an SPDX `A OR B` passes
//! when one side is allowed, `A AND B` only when both are, and a package with no
//! licence field fails.
//!
//! **The Desk computes nothing that decides**. A Frist, an amount,
//! a Werktag or a verdict is the service's; the browser renders it. So outside
//! `desk/src/format/` — the one layer that turns an instant or an amount into
//! text — no source builds or parses a date or a number. A second
//! implementation of a rule in TypeScript, disagreeing with the service on the
//! one screen a person acts from, is the failure this prevents.
//!
//! **Every word on screen comes from the catalogue.** German, with English
//! selectable: a view's text and accessible names come from `desk/src/i18n/`,
//! so a German string written into a `.tsx` file is the one an English screen
//! shows untranslated. The scan finds JSX text and a literal `aria-label`,
//! `title`, `placeholder`, `label`, `textValue` or `alt` in any `.tsx` file —
//! no file is exempt. In a `.ts` or `.tsx` module the scan also finds a text
//! property written as a literal — `label: "Neu buchen"`, `titel: "…"`,
//! `was: "…"` — and a code table naming a service's value in words
//! (`STORNO: "Storno"`): the tables a view reads its words from. The Desk's render test (`src/i18n/i18n.test.tsx`) holds
//! the views to the same rule at runtime, string literals in expressions
//! included.
//!
//! **The Desk's version is the workspace's.** It ships as the `mako-desk`
//! image beside the service images of one release, so `desk/package.json` and
//! its lockfile's root entries carry `[workspace.package] version`;
//! `bump-version` moves all three.

use std::path::Path;

/// Calls that construct or parse a date or a number. Outside `src/format/`
/// any of them is arithmetic on a value the service owns.
const COMPUTING: &[&str] = &[
    "new Date(",
    "Date.now(",
    "Date.parse(",
    "Date.UTC(",
    "Number(",
    "parseFloat(",
    "parseInt(",
    ".getTime(",
    "Temporal.",
];

/// The lowest number of packages and source files a healthy scan reads — below
/// it the paths have stopped matching and a clean result is about nothing.
const MIN_PACKAGES: usize = 50;
const MIN_SOURCES: usize = 10;

/// Words that are the same in every Sprache: the brand and the identifiers'
/// own names, which no catalogue translates.
const VERBATIM: &[&str] = &["mako", "Desk", "PID", "UTC", "MaLo", "ID"];

/// Props whose literal value is read out or shown.
const SHOWN_PROPS: &[&str] = &[
    "aria-label",
    "title",
    "placeholder",
    "label",
    "textValue",
    "alt",
];

pub fn run(root: &Path) -> bool {
    let allow = match allowed_licences(root) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("check-desk: {e}");
            return false;
        }
    };
    let Ok(lock) = std::fs::read_to_string(root.join("desk/package-lock.json")) else {
        eprintln!("check-desk: cannot read desk/package-lock.json");
        return false;
    };
    let (packages, refused) = match licence_violations(&lock, &allow) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("check-desk: desk/package-lock.json: {e}");
            return false;
        }
    };

    let mut sources = 0;
    let mut computing = Vec::new();
    let mut hardcoded = Vec::new();
    walk(&root.join("desk/src"), &mut |path, text| {
        sources += 1;
        let rel = path.strip_prefix(root).unwrap_or(path);
        computing.extend(computing_lines(rel, text));
        hardcoded.extend(hardcoded_strings(rel, text));
        hardcoded.extend(text_properties(rel, text));
    });

    let mut ok = true;
    for v in version_drift(root) {
        eprintln!("check-desk: {v}");
        ok = false;
    }
    if packages < MIN_PACKAGES || sources < MIN_SOURCES {
        eprintln!(
            "check-desk: read {packages} packages and {sources} sources — below the floor \
             ({MIN_PACKAGES}/{MIN_SOURCES}); the scan no longer matches the tree"
        );
        ok = false;
    }
    for r in &refused {
        eprintln!("check-desk: {r}");
        ok = false;
    }
    for c in &computing {
        eprintln!("check-desk: {c}");
        ok = false;
    }
    for h in &hardcoded {
        eprintln!("check-desk: {h}");
        ok = false;
    }
    if ok {
        println!(
            "check-desk: {packages} npm packages within deny.toml's licences; \
             {sources} sources compute no date or number outside src/format/; \
             every word on screen comes from the catalogue"
        );
    } else if !hardcoded.is_empty() {
        eprintln!(
            "check-desk: put the text into desk/src/i18n/de.ts and en.ts and read it with \
             useT() — the market's vocabulary stays German in both"
        );
    }
    if !ok && !computing.is_empty() {
        eprintln!(
            "check-desk: format a service's value in desk/src/format/ instead — \
             the Desk renders Fristen, amounts and verdicts, it never derives them"
        );
    }
    ok
}

/// Where the Desk's version differs from the workspace's.
fn version_drift(root: &Path) -> Vec<String> {
    let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default();
    let workspace = read("Cargo.toml")
        .split("[workspace.package]")
        .nth(1)
        .and_then(|t| {
            t.lines()
                .find(|l| l.trim_start().starts_with("version"))
                .and_then(|l| l.split('"').nth(1))
                .map(str::to_owned)
        });
    let Some(workspace) = workspace else {
        return vec!["no [workspace.package] version in Cargo.toml".to_owned()];
    };
    desk_versions(&read("desk/package.json"), &read("desk/package-lock.json"))
        .into_iter()
        .filter(|(_, v)| v.as_deref() != Some(workspace.as_str()))
        .map(|(where_, v)| {
            format!(
                "{where_} is {} but the workspace is {workspace} — `cargo xtask bump-version` moves both",
                v.as_deref().unwrap_or("unversioned")
            )
        })
        .collect()
}

/// The Desk's version in `package.json` and in both root entries of its lockfile.
fn desk_versions(package: &str, lock: &str) -> Vec<(&'static str, Option<String>)> {
    let field = |v: &serde_json::Value| v["version"].as_str().map(str::to_owned);
    let package: serde_json::Value = serde_json::from_str(package).unwrap_or_default();
    let lock: serde_json::Value = serde_json::from_str(lock).unwrap_or_default();
    vec![
        ("desk/package.json", field(&package)),
        ("desk/package-lock.json", field(&lock)),
        (
            "desk/package-lock.json packages[\"\"]",
            field(&lock["packages"][""]),
        ),
    ]
}

fn allowed_licences(root: &Path) -> Result<Vec<String>, String> {
    let text = std::fs::read_to_string(root.join("deny.toml"))
        .map_err(|e| format!("cannot read deny.toml: {e}"))?;
    let doc: toml::Value = text.parse().map_err(|e| format!("deny.toml: {e}"))?;
    let allow = doc
        .get("licenses")
        .and_then(|l| l.get("allow"))
        .and_then(toml::Value::as_array)
        .ok_or("deny.toml has no [licenses] allow list")?;
    Ok(allow
        .iter()
        .filter_map(toml::Value::as_str)
        .map(str::to_owned)
        .collect())
}

/// The number of packages in a lockfile and one line per package whose licence
/// the allow-list does not admit.
fn licence_violations(lock: &str, allow: &[String]) -> Result<(usize, Vec<String>), String> {
    let doc: serde_json::Value = serde_json::from_str(lock).map_err(|e| e.to_string())?;
    let packages = doc
        .get("packages")
        .and_then(serde_json::Value::as_object)
        .ok_or("no `packages` object (lockfile v3 expected)")?;
    let mut count = 0;
    let mut refused = Vec::new();
    for (path, pkg) in packages {
        if path.is_empty() || pkg.get("link").is_some() {
            continue;
        }
        count += 1;
        let name = path.rsplit("node_modules/").next().unwrap_or(path);
        match pkg.get("license").and_then(serde_json::Value::as_str) {
            None => refused.push(format!("{name}: no licence in the lockfile")),
            Some(expr) if !spdx_allowed(expr, allow) => refused.push(format!(
                "{name}: licence `{expr}` is not in deny.toml's [licenses] allow"
            )),
            Some(_) => {}
        }
    }
    Ok((count, refused))
}

/// Evaluate an SPDX expression against the allow-list: `OR` needs one side,
/// `AND` both; `X WITH Y` is one identifier, as `deny.toml` writes it.
fn spdx_allowed(expr: &str, allow: &[String]) -> bool {
    let spaced = expr.replace('(', " ( ").replace(')', " ) ");
    let mut tokens: Vec<String> = Vec::new();
    let mut words = spaced.split_whitespace().peekable();
    while let Some(w) = words.next() {
        if w == "WITH" {
            let exception = words.next().unwrap_or_default();
            if let Some(last) = tokens.last_mut() {
                *last = format!("{last} WITH {exception}");
            }
        } else {
            tokens.push(w.to_owned());
        }
    }
    let mut pos = 0;
    let result = or_expr(&tokens, &mut pos, allow);
    result && pos == tokens.len()
}

fn or_expr(t: &[String], pos: &mut usize, allow: &[String]) -> bool {
    let mut any = and_expr(t, pos, allow);
    while t.get(*pos).map(String::as_str) == Some("OR") {
        *pos += 1;
        any |= and_expr(t, pos, allow);
    }
    any
}

fn and_expr(t: &[String], pos: &mut usize, allow: &[String]) -> bool {
    let mut all = atom(t, pos, allow);
    while t.get(*pos).map(String::as_str) == Some("AND") {
        *pos += 1;
        all &= atom(t, pos, allow);
    }
    all
}

fn atom(t: &[String], pos: &mut usize, allow: &[String]) -> bool {
    match t.get(*pos).map(String::as_str) {
        Some("(") => {
            *pos += 1;
            let inner = or_expr(t, pos, allow);
            if t.get(*pos).map(String::as_str) == Some(")") {
                *pos += 1;
                inner
            } else {
                false
            }
        }
        Some(id) if id != ")" && id != "AND" && id != "OR" => {
            *pos += 1;
            allow.iter().any(|a| a == id)
        }
        _ => false,
    }
}

/// One line per computing call in a Desk source outside `src/format/`; test
/// files and comment lines are exempt.
fn computing_lines(rel: &Path, text: &str) -> Vec<String> {
    let path = rel.to_string_lossy().replace('\\', "/");
    if path.contains("/src/format/") || path.contains(".test.") || path.contains("/src/data/gen/") {
        return Vec::new();
    }
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let l = line.trim_start();
            !(l.starts_with("//") || l.starts_with('*') || l.starts_with("/*"))
        })
        .flat_map(|(n, line)| {
            COMPUTING
                .iter()
                .filter(|needle| line.contains(*needle))
                .map(|needle| format!("{path}:{}: `{needle}` outside src/format/", n + 1))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A word of shown text: two letters or more, and not one the catalogue keeps verbatim.
fn has_word(text: &str) -> bool {
    text.split(|c: char| !c.is_alphabetic())
        .any(|w| w.chars().count() >= 2 && !VERBATIM.contains(&w))
}

/// Text a TypeScript expression would carry rather than JSX text.
fn looks_like_code(text: &str) -> bool {
    text.starts_with(':')
        || ["&&", "||", "=", ";", "?", "`", "\"", "(", ")", "}"]
            .iter()
            .any(|m| text.contains(m))
}

/// The JSX text and literal shown props of a `.tsx` view or control. The
/// gallery (specimen data), tests and the catalogue itself are exempt.
fn hardcoded_strings(rel: &Path, text: &str) -> Vec<String> {
    let path = rel.to_string_lossy().replace('\\', "/");
    if !path.ends_with(".tsx")
        || path.contains(".test.")
        || path.contains("/src/gallery/")
        || path.contains("/src/test-support/")
        || path.contains("/src/i18n/")
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut previous = "";
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        let comment = l.starts_with("//") || l.starts_with('*') || l.starts_with("/*");
        if l.is_empty() || comment {
            continue;
        }
        let at = |what: &str| format!("{path}:{}: {what}", n + 1);
        // A shown prop with a literal: `aria-label="Konto"`.
        for prop in SHOWN_PROPS {
            let needle = format!(" {prop}=\"");
            let mut rest = line;
            while let Some(i) = rest.find(&needle) {
                let value = &rest[i + needle.len()..];
                let value = value.split('"').next().unwrap_or("");
                if has_word(value) {
                    out.push(at(&format!("{prop}=\"{value}\" is not from the catalogue")));
                }
                rest = &rest[i + needle.len()..];
            }
        }
        // JSX text on the line: `>Heute<`.
        let mut rest = line;
        while let Some(i) = rest.find('>') {
            let before = rest[..i].chars().last();
            rest = &rest[i + 1..];
            if before == Some('=') || before == Some('-') {
                continue;
            }
            let end = rest.find(['<', '{']).unwrap_or(rest.len());
            let shown = rest[..end].trim();
            if has_word(shown) && !looks_like_code(shown) {
                out.push(at(&format!("JSX text „{shown}“ is not from the catalogue")));
            }
        }
        // A line of JSX text under an opening tag: `<h1 …>` then `Heute`.
        let opens = previous.ends_with('>') && !previous.ends_with("=>");
        if opens && !l.starts_with(['<', '{', '}', ')', '/', '*']) {
            let end = l.find(['<', '{']).unwrap_or(l.len());
            let shown = l[..end].trim();
            if has_word(shown) && !looks_like_code(shown) {
                out.push(at(&format!("JSX text „{shown}“ is not from the catalogue")));
            }
        }
        previous = l;
    }
    out
}

/// Keys whose value is read out or shown: an object literal's `label: "…"` is
/// the text a view renders from a table.
const TEXT_KEYS: &[&str] = &[
    "label",
    "titel",
    "title",
    "untertitel",
    "frage",
    "was",
    "verb",
    "gehalten",
    "detail",
    "grund",
    "hinweis",
    "text",
    "name",
    "beschreibung",
];

/// The literal text properties of a `.ts`/`.tsx` module: `label: "Neu
/// buchen"`, or a code table's `STORNO: "Storno"` — an upper-case key, the
/// value a service returns, mapped to words. The catalogue, the generated
/// types, tests, the gallery's specimen data and the test support are exempt.
fn text_properties(rel: &Path, text: &str) -> Vec<String> {
    let path = rel.to_string_lossy().replace('\\', "/");
    if path.contains(".test.")
        || path.contains("/src/gallery/")
        || path.contains("/src/test-support/")
        || path.contains("/src/i18n/")
        || path.contains("/src/data/gen/")
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let l = line.trim();
        if l.starts_with("//") || l.starts_with('*') || l.starts_with("/*") {
            continue;
        }
        let Some((key, value)) = l.split_once(": \"") else {
            continue;
        };
        let key = key.trim_start_matches(['{', ' ', '(']).trim();
        let key = key.rsplit([' ', '{', ',']).next().unwrap_or(key);
        let Some(value) = value.split('"').next() else {
            continue;
        };
        let code_key = key.len() >= 2
            && key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        let shown = TEXT_KEYS.contains(&key) || code_key;
        // A code table's value is words when it is not the code itself.
        let words = has_word(value) && !(code_key && value == key);
        if shown && words && !looks_like_identifier(value) {
            out.push(format!(
                "{path}:{}: {key}: \"{value}\" is not from the catalogue",
                n + 1
            ));
        }
    }
    out
}

/// A value that names something rather than says it: a path, a URL, a
/// dotted or snake_case key, a CSS class list.
fn looks_like_identifier(value: &str) -> bool {
    value.contains(['/', '_', '.', '#', '@', '[', '{', '$'])
        || value.contains("--")
        || (!value.contains(' ') && value.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
}

fn walk(dir: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, visit);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("ts" | "tsx")
        ) && let Ok(text) = std::fs::read_to_string(&path)
        {
            visit(&path, &text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow() -> Vec<String> {
        ["MIT", "Apache-2.0", "ISC", "Apache-2.0 WITH LLVM-exception"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn a_desk_version_off_the_workspace_is_named() {
        let package = r#"{"name":"mako-desk","version":"0.24.0"}"#;
        let lock = r#"{"version":"0.24.1","packages":{"":{"version":"0.24.1"}}}"#;
        let found = desk_versions(package, lock);
        assert_eq!(found[0].1.as_deref(), Some("0.24.0"));
        assert_eq!(found[1].1.as_deref(), Some("0.24.1"));
        assert_eq!(found[2].1.as_deref(), Some("0.24.1"));
        assert_eq!(
            desk_versions("{}", "{}")[0].1,
            None,
            "an unversioned Desk is named too"
        );
    }

    #[test]
    fn spdx_or_needs_one_side_and_needs_both() {
        assert!(spdx_allowed("MIT", &allow()));
        assert!(spdx_allowed("(MIT OR GPL-3.0-only)", &allow()));
        assert!(!spdx_allowed("MIT AND GPL-3.0-only", &allow()));
        assert!(spdx_allowed("(MIT AND ISC) OR GPL-3.0-only", &allow()));
        assert!(spdx_allowed("Apache-2.0 WITH LLVM-exception", &allow()));
        assert!(!spdx_allowed(
            "GPL-2.0 WITH Classpath-exception-2.0",
            &allow()
        ));
        assert!(!spdx_allowed("", &allow()));
    }

    #[test]
    fn a_planted_copyleft_package_is_refused() {
        let lock = r#"{"packages": {
            "": {"name": "mako-desk"},
            "node_modules/fine": {"license": "MIT"},
            "node_modules/viral": {"license": "AGPL-3.0-only"},
            "node_modules/unknown": {}
        }}"#;
        let (count, refused) = licence_violations(lock, &allow()).expect("parses");
        assert_eq!(count, 3);
        assert_eq!(refused.len(), 2, "{refused:?}");
        assert!(refused.iter().any(|r| r.starts_with("viral:")));
        assert!(refused.iter().any(|r| r.starts_with("unknown:")));
    }

    #[test]
    fn a_planted_frist_computation_outside_format_is_refused() {
        let view = "const due = new Date(row.due_at);\nconst left = Date.now() - due;";
        let hits = computing_lines(Path::new("desk/src/views/Heute.tsx"), view);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits[0].contains("desk/src/views/Heute.tsx:1"));
    }

    #[test]
    fn format_tests_and_comments_are_exempt() {
        let text = "export const now = () => Date.now();";
        assert!(computing_lines(Path::new("desk/src/format/frist.ts"), text).is_empty());
        assert!(computing_lines(Path::new("desk/src/views/x.test.tsx"), text).is_empty());
        assert!(
            computing_lines(
                Path::new("desk/src/views/x.tsx"),
                "// a `new Date(` is wrong here"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_planted_german_string_in_a_view_is_refused() {
        let view = r#"export function V() {
  return (
    <section aria-label="Angaben">
      <h1 className="m-0">
        Fristen überschritten
      </h1>
      <p>Keine Frist.</p>
    </section>
  );
}"#;
        let hits = hardcoded_strings(Path::new("desk/src/views/V.tsx"), view);
        assert_eq!(hits.len(), 3, "{hits:?}");
        assert!(hits[0].contains(":3: aria-label=\"Angaben\""));
        assert!(hits[1].contains(":5: JSX text „Fristen überschritten“"));
        assert!(hits[2].contains(":7: JSX text „Keine Frist.“"));
    }

    #[test]
    fn catalogue_reads_code_and_verbatim_words_pass() {
        let view = r#"export function V({ p }: { p: Row }) {
  const t = useT();
  const xs = useState<string>("");
  return (
    <section aria-label={t.liste.angaben}>
      <h1>{t.fristen.titel}</h1>
      {p.ok ? <Ja /> : undefined}
      <span>PID {p.pid}</span>
      {rows.map((r) =>
        r.name
      )}
    </section>
  );
}"#;
        let hits = hardcoded_strings(Path::new("desk/src/views/V.tsx"), view);
        assert!(hits.is_empty(), "{hits:?}");
        let gallery = r#"<p title="Hell">Dunkel</p>"#;
        assert!(hardcoded_strings(Path::new("desk/src/gallery/G.tsx"), gallery).is_empty());
        assert!(hardcoded_strings(Path::new("desk/src/views/v.test.tsx"), gallery).is_empty());
    }

    #[test]
    fn an_amount_parsed_to_a_number_is_refused() {
        let hits = computing_lines(
            Path::new("desk/src/views/Rechnung.tsx"),
            "const total = parseFloat(a) + Number(b);",
        );
        assert_eq!(hits.len(), 2, "{hits:?}");
    }

    /// A text property planted in a plain module is refused by its key, and
    /// a code table mapping a service's value to words by its upper-case key;
    /// a key, a path or the catalogue itself passes.
    #[test]
    fn a_planted_text_property_in_a_module_is_refused() {
        let planted = "export const AKTION = {\n  verb: \"Neu buchen\",\n  \
                       label: \"Bemerkung (optional)\",\n};\n\
                       const ART: Record<string, string> = {\n  STORNO: \"Storno\",\n  \
                       SEPA_STORNO: \"SEPA-Storno\",\n};\n";
        let hits = text_properties(Path::new("desk/src/app/aktionen.ts"), planted);
        assert_eq!(hits.len(), 4, "{hits:#?}");
        assert!(hits[0].contains("verb: \"Neu buchen\""), "{hits:#?}");
        assert!(hits[2].contains("STORNO: \"Storno\""), "{hits:#?}");
        let clean = "const Q = {\n  queryKey: \"x\",\n  name: \"get_process\",\n  \
                     label: \"tool/name\",\n  RECHNUNG: \"RECHNUNG\",\n  \
                     title: katalog().x,\n};\n";
        assert!(
            text_properties(Path::new("desk/src/app/q.ts"), clean).is_empty(),
            "{:#?}",
            text_properties(Path::new("desk/src/app/q.ts"), clean)
        );
        assert!(text_properties(Path::new("desk/src/i18n/de.ts"), planted).is_empty());
        assert!(text_properties(Path::new("desk/src/app/a.test.ts"), planted).is_empty());
    }
}
