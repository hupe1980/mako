//! `check-docker-build` — the Dockerfile's builder stage compiles the services
//! in one `cargo build`, over the package list `cargo chef cook` prepared.
//!
//! Cargo unifies dependency features per invocation. A second `cargo build`
//! over a different package set compiles every shared dependency whose feature
//! set differs a second time, and a release build with LTO pushes the builder
//! job past its timeout. A package cooked but not built, or built but not
//! copied out, is the same drift seen from the other side.

use std::collections::BTreeSet;
use std::path::Path;

/// Runs the check against the workspace's `Dockerfile`.
#[must_use]
pub fn run(root: &Path) -> bool {
    let path = root.join("Dockerfile");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("check-docker-build: cannot read {}: {e}", path.display());
            return false;
        }
    };
    let problems = problems(&text);
    if problems.is_empty() {
        println!("check-docker-build: one cargo build over the cooked package list");
        return true;
    }
    eprintln!("check-docker-build: Dockerfile builder stage:");
    for p in &problems {
        eprintln!("  - {p}");
    }
    false
}

/// The Dockerfile with line continuations joined, one logical line per entry.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if line.trim_start().starts_with('#') {
            continue;
        }
        match line.trim_end().strip_suffix('\\') {
            Some(head) => {
                cur.push_str(head);
                cur.push(' ');
            }
            None => {
                cur.push_str(line);
                out.push(std::mem::take(&mut cur));
            }
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The `-p` packages of the command starting at `marker`, up to the next `&&`.
fn packages(line: &str, marker: &str) -> Vec<BTreeSet<String>> {
    line.match_indices(marker)
        .map(|(i, _)| {
            let cmd = line[i..].split("&&").next().unwrap_or_default();
            let words: Vec<&str> = cmd.split_whitespace().collect();
            words
                .windows(2)
                .filter(|w| w[0] == "-p")
                .map(|w| w[1].to_owned())
                .collect()
        })
        .collect()
}

/// The binaries named by the `for bin in … ; do` loop.
fn copied(line: &str) -> Option<BTreeSet<String>> {
    let rest = &line[line.find("for bin in ")? + "for bin in ".len()..];
    let list = rest.split(';').next()?;
    Some(list.split_whitespace().map(str::to_owned).collect())
}

fn problems(text: &str) -> Vec<String> {
    let lines = logical_lines(text);
    let cooked: Vec<_> = lines
        .iter()
        .flat_map(|l| packages(l, "cargo chef cook"))
        .collect();
    let built: Vec<_> = lines
        .iter()
        .flat_map(|l| packages(l, "cargo auditable build"))
        .collect();
    let copies: Vec<_> = lines.iter().filter_map(|l| copied(l)).collect();

    let mut out = Vec::new();
    if cooked.len() != 1 {
        out.push(format!(
            "expected one `cargo chef cook`, found {}",
            cooked.len()
        ));
    }
    if built.len() != 1 {
        out.push(format!(
            "expected one `cargo auditable build`, found {} — Cargo unifies features per \
             invocation, so a second build recompiles the shared dependencies",
            built.len()
        ));
    }
    if copies.len() != 1 {
        out.push(format!(
            "expected one `for bin in …` copy loop, found {}",
            copies.len()
        ));
    }
    if let (Some(c), Some(b)) = (cooked.first(), built.first())
        && c != b
    {
        out.push(format!(
            "cooked and built packages differ: cooked only {:?}, built only {:?}",
            c.difference(b).collect::<Vec<_>>(),
            b.difference(c).collect::<Vec<_>>()
        ));
    }
    if let (Some(b), Some(k)) = (built.first(), copies.first())
        && b != k
    {
        out.push(format!(
            "built packages and copied binaries differ: built only {:?}, copied only {:?}",
            b.difference(k).collect::<Vec<_>>(),
            k.difference(b).collect::<Vec<_>>()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "\
# cargo chef cook in a comment -p ignored
RUN --mount=type=cache,target=/x \\
    cargo chef cook --profile ${PROFILE} \\
                    -p a -p b \\
                    --recipe-path recipe.json
RUN cargo auditable build --profile ${PROFILE} \\
                -p a -p b \\
    && for bin in a b; do \\
         cp x y; \\
       done
";

    #[test]
    fn one_build_over_the_cooked_list_passes() {
        assert!(problems(GOOD).is_empty(), "{:?}", problems(GOOD));
    }

    #[test]
    fn a_second_build_is_refused() {
        let bad = GOOD.replace(
            "    && for bin",
            "    && cargo auditable build --profile ${PROFILE} -p b --features x \\\n    && for bin",
        );
        let p = problems(&bad);
        assert!(p.iter().any(|m| m.contains("found 2")), "{p:?}");
    }

    #[test]
    fn a_package_built_but_not_cooked_is_refused() {
        let bad = GOOD.replacen(
            "-p a -p b \\\n    && for",
            "-p a -p b -p c \\\n    && for",
            1,
        );
        let p = problems(&bad);
        assert!(p.iter().any(|m| m.contains("cooked and built")), "{p:?}");
        assert!(p.iter().any(|m| m.contains("copied")), "{p:?}");
    }

    #[test]
    fn the_real_dockerfile_passes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let text = std::fs::read_to_string(root.join("Dockerfile")).unwrap();
        assert!(problems(&text).is_empty(), "{:?}", problems(&text));
    }
}
