//! Guard: a service's `main.rs` is the one-line runner call.
//!
//! A service is a library: its `Daemon` lives in `lib.rs` (or a module of it)
//! and `main.rs` is
//!
//! ```rust,ignore
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     mako_service::run::<svc::Daemon>().await
//! }
//! ```
//!
//! because a test under `services/<svc>/tests/it` can import a library and
//! cannot import a binary. Wiring kept in `main.rs` — the `Daemon` impl, the
//! router assembly, a worker spawn — is the code that ships and the code no
//! integration test can build, so a route colliding with `/health` or a
//! missing Cedar layer shows up only in a running process.
//!
//! What may stand beside the call: comments (`//`, `//!`, `///`), blank lines
//! and one-line inner attributes (`#![deny(unsafe_code)]`, which
//! `check-crate-lints` requires of every crate root). Anything else refuses the
//! file.
//!
//! Every runner daemon's `main.rs` is that call; [`EXEMPT`] names the one
//! service that drives its own `main`, and an entry whose `main.rs` has become
//! thin fails until it is removed, so the list cannot hold a stale claim.

use std::path::Path;

/// Services that drive their own `main` and are no `mako_service::Daemon`.
const EXEMPT: &[(&str, &str)] = &[(
    "makod",
    "drives its own `main` and takes `mako-service` piecemeal; it has no \
     `Daemon` impl for `run` to call",
)];

/// The lowest number of `services/*/src/main.rs` a healthy scan finds — below
/// it the walk has stopped matching and a clean result is about nothing.
const MIN_MAINS: usize = 10;

/// `src` is the runner call and nothing else that is not a comment, a blank
/// line or a one-line inner attribute.
pub fn is_thin(src: &str) -> bool {
    let body: String = src
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//") && !l.starts_with("#!["))
        .collect::<String>()
        .split_whitespace()
        .collect();
    let Some(rest) =
        body.strip_prefix("#[tokio::main]asyncfnmain()->anyhow::Result<()>{mako_service::run::<")
    else {
        return false;
    };
    let Some(path) = rest.strip_suffix(">().await}") else {
        return false;
    };
    !path.is_empty()
        && path.split("::").all(|seg| {
            !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The services whose `main.rs` is not thin, sorted, and the number of mains
/// read.
fn scan(root: &Path) -> (Vec<String>, usize) {
    let mut fat = Vec::new();
    let mut read = 0;
    let mut dirs: Vec<_> = std::fs::read_dir(root.join("services"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        let Ok(src) = std::fs::read_to_string(dir.join("src/main.rs")) else {
            continue;
        };
        read += 1;
        if !is_thin(&src) {
            let name = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            fat.push(name);
        }
    }
    (fat, read)
}

/// What the scan says about `fat` against [`EXEMPT`].
fn findings(fat: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for svc in fat {
        let exempt = EXEMPT.iter().any(|(n, _)| n == svc);
        if !exempt {
            out.push(format!(
                "services/{svc}/src/main.rs is more than `mako_service::run::<D>().await` — move \
                 the `Daemon` and everything around it into the library (`lib.rs` or a module \
                 of it)"
            ));
        }
    }
    for (svc, _) in EXEMPT {
        if !fat.iter().any(|f| f == svc) {
            out.push(format!(
                "services/{svc}/src/main.rs is thin (or gone) — remove `{svc}` from EXEMPT in \
                 xtask/src/check_thin_main.rs"
            ));
        }
    }
    out
}

pub fn run(root: &Path) -> bool {
    let (fat, read) = scan(root);
    if read < MIN_MAINS {
        eprintln!(
            "check-thin-main: read {read} services/*/src/main.rs, at least {MIN_MAINS} expected \
             — the walk has stopped matching"
        );
        return false;
    }
    let out = findings(&fat);
    if out.is_empty() {
        println!(
            "check-thin-main: {} of {read} service mains are the runner call; {} exempt",
            read - fat.len(),
            EXEMPT.len()
        );
        return true;
    }
    for f in &out {
        eprintln!("check-thin-main: {f}");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const THIN: &str = "#![deny(unsafe_code)]\n\n//! `invoicd` — the daemon.\n\n\
        #[tokio::main]\nasync fn main() -> anyhow::Result<()> {\n    \
        mako_service::run::<invoicd::server::Invoicd>().await\n}\n";

    #[test]
    fn the_runner_call_is_thin() {
        assert!(is_thin(THIN));
        assert!(is_thin(
            "#[tokio::main]\nasync fn main() -> anyhow::Result<()> { mako_service::run::<x::X>().await }"
        ));
    }

    #[test]
    fn a_daemon_impl_in_main_is_refused() {
        let fat = THIN.replace(
            "#[tokio::main]",
            "struct D;\nimpl mako_service::Daemon for D {}\n#[tokio::main]",
        );
        assert!(!is_thin(&fat));
        assert!(!is_thin(
            &THIN.replace("}\n", "}\n#[cfg(test)]\nmod tests {}\n")
        ));
        assert!(!is_thin(&THIN.replace(".await", ".await?;\n    Ok(())")));
        assert!(!is_thin(&THIN.replace("run::<", "run_with::<")));
        assert!(!is_thin(""));
    }

    #[test]
    fn a_new_fat_main_is_named_and_a_thinned_exemption_is_stale() {
        let mut fat: Vec<String> = vec!["makod".into()];
        assert!(findings(&fat).is_empty());

        fat.push("newd".into());
        let out = findings(&fat);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("services/newd/src/main.rs"), "{out:?}");

        let out = findings(&[]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("remove `makod`"), "{out:?}");
    }

    /// Today's tree: every main but the exemption is the runner call, and the
    /// exemption is fat — the guard can fail on what it guards.
    #[test]
    fn the_real_tree_matches_the_exemption() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        let (fat, read) = scan(root);
        assert!(read >= MIN_MAINS, "{read}");
        let expected: Vec<String> = EXEMPT.iter().map(|(n, _)| (*n).to_owned()).collect();
        assert_eq!(fat, expected);
        assert!(findings(&fat).is_empty(), "{:?}", findings(&fat));
    }
}
