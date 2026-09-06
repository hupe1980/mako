//! Every item the published prose imports still exists — the root `README.md`
//! and the documentation site.
//!
//! No README in this workspace is included with `#[doc = include_str!(…)]` and
//! nothing compiles the site's snippets either, so a renamed type leaves the
//! front page of the project — and 74 code blocks across the docs — telling a
//! reader to write code that does not build, with every gate green.
//!
//! What this checks is **name resolution**, not compilation. The snippets are
//! illustrative fragments — `let msg = parse(bytes)?;` names a `bytes` the page
//! never declares — so requiring them to build would mean filling the README
//! with scaffolding a reader has to skip. What actually rots is the API surface
//! they name, and that is what is pinned here: every crate a snippet imports is
//! a workspace member, and every item it names is `pub` somewhere in that
//! crate's sources.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The workspace root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("services/makod has a workspace above it")
        .to_path_buf()
}

/// Every ```` ```rust ```` block of a Markdown file, in order.
fn rust_blocks(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        match (&mut current, line.trim_start()) {
            (None, fence) if fence.starts_with("```rust") => current = Some(String::new()),
            (Some(_), fence) if fence.starts_with("```") => {
                out.push(current.take().unwrap_or_default());
            }
            (Some(buf), _) => {
                buf.push_str(line);
                buf.push('\n');
            }
            (None, _) => {}
        }
    }
    out
}

/// `use a::b::{C, D as E};` → `("a", ["C", "D"])`. A glob or a bare module
/// import names no item and yields none.
fn imported_items(line: &str) -> Option<(String, Vec<String>)> {
    let rest = line.trim().strip_prefix("use ")?.trim_end_matches(';');
    let krate = rest.split([':', '{', ' ']).next()?.to_owned();
    let names = match rest.split_once('{') {
        Some((_, list)) => list
            .trim_end_matches('}')
            .split(',')
            .filter_map(|n| n.split(" as ").next())
            // A brace list may carry a nested path (`builders::UtilmdBuilder`);
            // the item is its last segment.
            .filter_map(|n| n.trim().rsplit("::").next())
            .map(str::trim)
            .filter(|n| !n.is_empty() && *n != "self")
            .map(ToOwned::to_owned)
            .collect(),
        None => rest
            .rsplit("::")
            .next()
            .map(|n| vec![n.trim().to_owned()])
            .unwrap_or_default(),
    };
    // Only the names that look like items: a lower-case tail is a module or a
    // function, and a function is checked the same way a type is.
    let names = names
        .into_iter()
        .filter(|n| n != "*" && !n.is_empty())
        .collect();
    Some((krate, names))
}

/// Every `pub` item name declared in a crate's sources.
fn public_items(src: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // `pub use a::b::{\n C,\n D,\n};` spans lines, so re-exports are read
            // off the whole text up to their `;` rather than line by line.
            let mut rest = text.as_str();
            while let Some(i) = rest.find("pub use ") {
                rest = &rest[i + "pub use ".len()..];
                let Some(end) = rest.find(';') else { break };
                let stmt: String = rest[..end].split_whitespace().collect::<Vec<_>>().join(" ");
                if let Some((_, names)) = imported_items(&format!("use {stmt};")) {
                    out.extend(names);
                }
                rest = &rest[end..];
            }
            for line in text.lines() {
                let t = line.trim();
                let Some(after) = t.strip_prefix("pub ") else {
                    continue;
                };
                if after.starts_with("use ") {
                    continue;
                }
                let after = after
                    .trim_start_matches("async ")
                    .trim_start_matches("const ")
                    .trim_start_matches("unsafe ")
                    .trim_start_matches("extern \"C\" ");
                for kw in [
                    "struct ", "enum ", "trait ", "fn ", "type ", "mod ", "static ",
                ] {
                    if let Some(name) = after.strip_prefix(kw) {
                        let name: String = name
                            .chars()
                            .take_while(|c| c.is_alphanumeric() || *c == '_')
                            .collect();
                        if !name.is_empty() {
                            out.insert(name);
                        }
                        break;
                    }
                }
            }
        }
    }
    out
}

/// The `<pre><code class="language-rust">` block the landing page carries.
///
/// It is a Tera template rather than Markdown, so `rust_blocks` does not see
/// it — and it is the single most-read snippet in the project.
fn landing_page_blocks(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("<code class=\"language-rust\">") {
        rest = &rest[i..];
        let Some(start) = rest.find('>') else { break };
        let Some(end) = rest.find("</code>") else {
            break;
        };
        out.push(rest[start + 1..end].to_owned());
        rest = &rest[end..];
    }
    out
}

/// Crate names the prose may import without being workspace members.
///
/// Every dependency table in the tree — the workspace one and each member's,
/// since a crate only `energy-api` takes (`url`) is declared there and nowhere
/// else. Read from the manifests rather than listed here, so a snippet citing a
/// **renamed workspace crate** still fails: it would be neither a member nor a
/// declared dependency of anything.
fn external_dependencies(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut manifests = vec![root.join("Cargo.toml")];
    for dir in ["crates", "services", "xtask"] {
        let base = root.join(dir);
        if base.join("Cargo.toml").is_file() {
            manifests.push(base.join("Cargo.toml"));
        }
        for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let candidate = entry.path().join("Cargo.toml");
            if candidate.is_file() {
                manifests.push(candidate);
            }
        }
    }
    for path in manifests {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut in_deps = false;
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with('[') {
                in_deps = t.contains("dependencies]");
                continue;
            }
            if !in_deps || t.starts_with('#') {
                continue;
            }
            if let Some((name, _)) = t.split_once('=') {
                let name = name.trim().replace('-', "_");
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    out.insert(name);
                }
            }
        }
    }
    out
}

/// Hold every item the given snippets import to a `pub` declaration, returning
/// how many crates they named.
///
/// `source` names the document in the failure, because the point of the guard
/// is to send someone to the page that has rotted. A page whose snippets import
/// nothing is not an error — plenty are fragments that continue an example
/// above them — so the "did the scan see anything at all" check belongs to the
/// caller, over the whole set.
fn assert_imports_resolve(root: &Path, source: &str, blocks: &[String]) -> usize {
    let mut wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in blocks.iter().flat_map(|b| b.lines()) {
        let Some((krate, names)) = imported_items(line) else {
            continue;
        };
        if krate == "std" || krate == "crate" || krate == "super" {
            continue;
        }
        wanted.entry(krate).or_default().extend(names);
    }
    let found = wanted.len();
    let external = external_dependencies(root);
    for (krate, names) in wanted {
        let dir = krate.replace('_', "-");
        let src = [
            root.join("crates").join(&dir),
            root.join("services").join(&dir),
        ]
        .into_iter()
        .map(|p| p.join("src"))
        .find(|p| p.is_dir());
        let Some(src) = src else {
            // An external crate's surface is pinned by `cargo check`, not here.
            assert!(
                external.contains(&krate),
                "{source} imports `{krate}`, which is neither a member of this workspace \
                 nor a declared dependency — either the crate was renamed or removed, or \
                 the snippet is citing something that never shipped"
            );
            continue;
        };
        let available = public_items(&src);
        for name in names {
            // A lower-case name is a module or a free function; both are `pub`
            // declarations and both are covered by the scan.
            assert!(
                available.contains(&name),
                "{source} imports `{krate}::…::{name}`, and no `pub` item of that name \
                 exists in {}",
                src.display()
            );
        }
    }
    found
}

/// The landing page and every page of the documentation site.
///
/// The site is where a reader arrives from a search result, so a snippet naming
/// a type that no longer exists is read by more people than the README's.
#[test]
fn every_item_the_site_docs_import_exists() {
    let root = workspace_root();

    let landing = root.join("site/templates/index.html");
    let html = std::fs::read_to_string(&landing).expect("the landing page");
    let blocks = landing_page_blocks(&html);
    assert_eq!(
        blocks.len(),
        1,
        "the landing page's Rust snippet moved or was dropped"
    );
    let mut imports = assert_imports_resolve(&root, "the landing page", &blocks);
    assert!(imports > 0, "the landing page's snippet imports nothing");

    let mut pages = 0usize;
    let mut stack = vec![root.join("site/content")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("the site content tree")
            .flatten()
        {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a site page");
            let blocks = rust_blocks(&text);
            if blocks.is_empty() {
                continue;
            }
            pages += 1;
            let name = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            imports += assert_imports_resolve(&root, &name, &blocks);
        }
    }
    assert!(
        pages >= 10 && imports >= 10,
        "only {pages} site pages carry Rust snippets, naming {imports} crate(s) — \
         the scan stopped seeing them"
    );
}

#[test]
fn every_item_the_readme_imports_exists() {
    let root = workspace_root();
    let readme = std::fs::read_to_string(root.join("README.md")).expect("the root README");
    let blocks = rust_blocks(&readme);
    assert!(
        blocks.len() >= 9,
        "the README lost its Rust snippets — found {}",
        blocks.len()
    );

    let mut wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for line in blocks.iter().flat_map(|b| b.lines()) {
        let Some((krate, names)) = imported_items(line) else {
            continue;
        };
        if krate == "std" || krate == "crate" || krate == "super" {
            continue;
        }
        wanted.entry(krate).or_default().extend(names);
    }
    assert!(!wanted.is_empty(), "the snippets import nothing at all");

    for (krate, names) in wanted {
        let dir = krate.replace('_', "-");
        let src = [
            root.join("crates").join(&dir),
            root.join("services").join(&dir),
        ]
        .into_iter()
        .map(|p| p.join("src"))
        .find(|p| p.is_dir());
        let Some(src) = src else {
            panic!(
                "the README imports `{krate}`, which is not a member of this workspace — \
                 either the crate was renamed or removed, or the snippet is citing something \
                 that never shipped"
            );
        };
        let available = public_items(&src);
        for name in names {
            // A lower-case name is a module or a free function; both are `pub`
            // declarations and both are covered by the scan.
            assert!(
                available.contains(&name),
                "the README's snippet imports `{krate}::…::{name}`, and no `pub` item of \
                 that name exists in {}",
                src.display()
            );
        }
    }
}
