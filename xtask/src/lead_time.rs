//! `cargo xtask lead-time` — how long a BDEW release takes to reach the tree.
//!
//! Per profile in `crates/edi-energy/profiles/sources.json`, three dates: the
//! release's `publikationsdatum`, the first commit that names the profile's key
//! in `sources.json`, and its `valid_from`. The reading prints the days from
//! publication to commit and from commit to `valid_from`; a commit later than
//! `valid_from` shows as a negative second figure.
//!
//! The first commit comes from the pickaxe (`git log -S'"<key>"' --reverse`),
//! not from `--diff-filter=A`: every profile shares the one file, so the file's
//! creation would answer for all of them.
//!
//! The reading is forward-only. Profiles that were first committed at or before
//! **the cut** — the commit that added this file, or `--cut <rev>` — are listed
//! and not measured: the repository's history opens after most of them were
//! binding, so their figure would measure the repository's age, not a lead
//! time. A shallow clone has no first commit to find and is refused.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use time::Date;

/// The profile catalogue, relative to the repository root.
const SOURCES: &str = "crates/edi-energy/profiles/sources.json";
/// This file, whose first commit is the default cut.
const SELF_PATH: &str = "xtask/src/lead_time.rs";

#[derive(Deserialize)]
struct Sources {
    profiles: std::collections::BTreeMap<String, Entry>,
}

#[derive(Deserialize)]
struct Entry {
    valid_from: String,
    #[serde(default)]
    publikationsdatum: Option<String>,
}

/// One profile's reading.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    key: String,
    publikation: Option<Date>,
    first_commit: Date,
    valid_from: Date,
    /// First committed after the cut, so its figures count.
    measured: bool,
}

impl Row {
    /// Days from publication to the first commit.
    fn publication_to_commit(&self) -> Option<i64> {
        self.publikation
            .map(|p| (self.first_commit - p).whole_days())
    }

    /// Days from the first commit to `valid_from` — negative when the profile
    /// arrived after it was already binding.
    fn commit_to_valid_from(&self) -> i64 {
        (self.valid_from - self.first_commit).whole_days()
    }
}

fn parse_date(s: &str) -> Result<Date, String> {
    Date::parse(s, time::macros::format_description!("[year]-[month]-[day]"))
        .map_err(|e| format!("{s:?} is not a date: {e}"))
}

fn git(repo: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(repo, args)?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The commit profiles must postdate to be measured.
fn resolve_cut(repo: &Path, cut: Option<&str>) -> Result<String, String> {
    if let Some(rev) = cut {
        return git_text(
            repo,
            &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
        );
    }
    let added = git_text(
        repo,
        &["log", "--diff-filter=A", "--format=%H", "--", SELF_PATH],
    )?;
    match added.lines().last() {
        Some(h) if !h.is_empty() => Ok(h.to_owned()),
        // Not committed yet: everything in the history predates it.
        _ => git_text(repo, &["rev-parse", "HEAD"]),
    }
}

/// The reading for every profile in `repo`'s `sources.json`.
fn reading(repo: &Path, cut: Option<&str>) -> Result<Vec<Row>, String> {
    if git_text(repo, &["rev-parse", "--is-shallow-repository"])? == "true" {
        return Err(
            "the clone is shallow, so a profile's first commit cannot be found — \
             run `git fetch --unshallow` first"
                .to_owned(),
        );
    }
    let cut = resolve_cut(repo, cut)?;
    let text =
        std::fs::read_to_string(repo.join(SOURCES)).map_err(|e| format!("read {SOURCES}: {e}"))?;
    let sources: Sources =
        serde_json::from_str(&text).map_err(|e| format!("parse {SOURCES}: {e}"))?;

    let mut rows = Vec::new();
    for (key, entry) in sources.profiles {
        let pickaxe = format!("-S\"{key}\"");
        let log = git_text(
            repo,
            &[
                "log",
                &pickaxe,
                "--reverse",
                "--format=%H %cs",
                "--",
                SOURCES,
            ],
        )?;
        let Some((hash, date)) = log.lines().next().and_then(|l| l.split_once(' ')) else {
            // In the working tree but not committed: nothing to measure yet.
            continue;
        };
        let before_cut = git(repo, &["merge-base", "--is-ancestor", hash, &cut])?
            .status
            .success();
        rows.push(Row {
            publikation: entry
                .publikationsdatum
                .as_deref()
                .map(parse_date)
                .transpose()?,
            first_commit: parse_date(date)?,
            valid_from: parse_date(&entry.valid_from)?,
            measured: !before_cut,
            key,
        });
    }
    Ok(rows)
}

/// Run the reading. Returns `false` when it cannot be taken.
pub fn run(workspace_root: &Path, args: &[String]) -> bool {
    let cut = args
        .iter()
        .position(|a| a == "--cut")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    let rows = match reading(workspace_root, cut) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("lead-time: {e}");
            return false;
        }
    };
    let (measured, before): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| r.measured);
    if measured.is_empty() {
        println!("lead-time: no profile has been added since the cut");
    } else {
        println!(
            "{:<22} {:<11} {:<12} {:<11} {:>12} {:>18}",
            "profile",
            "publikation",
            "first commit",
            "valid_from",
            "pub → commit",
            "commit → valid_from"
        );
        for r in &measured {
            println!(
                "{:<22} {:<11} {:<12} {:<11} {:>12} {:>18}",
                r.key,
                r.publikation
                    .map_or_else(|| "—".to_owned(), |d| d.to_string()),
                r.first_commit,
                r.valid_from,
                r.publication_to_commit()
                    .map_or_else(|| "—".to_owned(), |d| d.to_string()),
                r.commit_to_valid_from(),
            );
        }
    }
    println!(
        "\n{} profile(s) predate the cut and are not measured: {}",
        before.len(),
        before
            .iter()
            .map(|r| r.key.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A throwaway git repository under the system temp directory.
    struct Fixture(PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("mako-lead-time-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("crates/edi-energy/profiles")).unwrap();
            let f = Self(dir);
            f.git(&["init", "-q", "-b", "main"], "2026-01-01");
            f
        }

        fn git(&self, args: &[&str], date: &str) {
            let stamp = format!("{date}T12:00:00+00:00");
            let ok = Command::new("git")
                .arg("-C")
                .arg(&self.0)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.org"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .env("GIT_AUTHOR_DATE", &stamp)
                .env("GIT_COMMITTER_DATE", &stamp)
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        }

        /// Commit `sources.json` holding `profiles` on `date`; returns the hash.
        fn commit(&self, profiles: &[(&str, &str, Option<&str>)], date: &str) -> String {
            let body: Vec<String> = profiles
                .iter()
                .map(|(k, v, p)| {
                    let p = p.map_or(String::new(), |p| {
                        format!(r#", "publikationsdatum": "{p}""#)
                    });
                    format!(r#""{k}": {{"valid_from": "{v}"{p}}}"#)
                })
                .collect();
            std::fs::write(
                self.0.join(SOURCES),
                format!("{{\"profiles\": {{{}}}}}\n", body.join(",\n")),
            )
            .unwrap();
            self.git(&["add", "."], date);
            self.git(&["commit", "-q", "-m", date], date);
            git_text(&self.0, &["rev-parse", "HEAD"]).unwrap()
        }
    }

    const OLD: (&str, &str, Option<&str>) = ("utilts/fv20250401", "2025-04-01", Some("2024-10-01"));
    const NEW: (&str, &str, Option<&str>) = ("utilmd/fv20271001", "2027-10-01", Some("2027-04-01"));
    const LATE: (&str, &str, Option<&str>) =
        ("mscons/fv20271001", "2027-10-01", Some("2027-04-01"));

    /// A profile added after the cut is measured publication → commit → binding
    /// in days; one committed at or before the cut is listed, not measured.
    #[test]
    fn a_profile_after_the_cut_is_measured_and_one_before_is_not() {
        let f = Fixture::new("measured");
        let cut = f.commit(&[OLD], "2026-08-24");
        f.commit(&[OLD, NEW], "2027-04-20");

        let rows = reading(&f.0, Some(&cut)).unwrap();
        let old = rows.iter().find(|r| r.key == OLD.0).unwrap();
        assert!(
            !old.measured,
            "a profile at the cut measures the repository's age"
        );
        let new = rows.iter().find(|r| r.key == NEW.0).unwrap();
        assert!(new.measured);
        assert_eq!(new.publication_to_commit(), Some(19));
        assert_eq!(new.commit_to_valid_from(), 164);
    }

    /// A profile first committed after its `valid_from` reads negative, so a
    /// late import shows in the same reading rather than vanishing from it.
    #[test]
    fn a_commit_after_valid_from_reads_negative() {
        let f = Fixture::new("late");
        let cut = f.commit(&[OLD], "2026-08-24");
        f.commit(&[OLD, NEW], "2027-04-20");
        f.commit(&[OLD, NEW, LATE], "2027-10-05");

        let rows = reading(&f.0, Some(&cut)).unwrap();
        let late = rows.iter().find(|r| r.key == LATE.0).unwrap();
        assert!(late.measured);
        assert_eq!(late.commit_to_valid_from(), -4);
        // The pickaxe finds the commit that added the key, not the latest edit
        // of the shared file.
        let new = rows.iter().find(|r| r.key == NEW.0).unwrap();
        assert_eq!(new.first_commit, parse_date("2027-04-20").unwrap());
    }

    /// A shallow clone would answer every profile with its own grafted root,
    /// so the reading refuses it rather than report a wrong figure.
    #[test]
    fn a_shallow_clone_is_refused() {
        let f = Fixture::new("shallow-src");
        f.commit(&[OLD], "2026-08-24");
        f.commit(&[OLD, NEW], "2027-04-20");
        let clone =
            std::env::temp_dir().join(format!("mako-lead-time-shallow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&clone);
        let ok = Command::new("git")
            .args(["clone", "-q", "--depth", "1"])
            .arg(format!("file://{}", f.0.display()))
            .arg(&clone)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let shallow = Fixture(clone);
        let err = reading(&shallow.0, None).unwrap_err();
        assert!(err.contains("shallow"), "{err}");
    }
}
