//! Guard: the feature specifications and the roadmap agree.
//!
//! `concepts/ROADMAP.md` holds the direction and the order; each milestone
//! `M<n>` has one row, and that row links the specification under
//! `specs/NNN-name/` that carries the detail. Two lists that name each other
//! drift unless something holds them together, so this check refuses:
//!
//! - a spec whose `**Status**:` disagrees with what its `tasks.md` derives
//!   ([`derived_status`]);
//! - a `Parked` spec the roadmap's parked list does not name — parking is a
//!   decision the roadmap records, and that list is where the scheduling wait
//!   lives. A parked spec's tasks keep their own markers: `[ ]` for work that
//!   is buildable once scheduled, `[~]` only for a wait on somebody who is not
//!   us, so its Status is not derived from them;
//! - a spec without `plan.md` or `tasks.md` — the workflow is specify → plan →
//!   tasks;
//! - a spec `NNN` that does not cite its own milestone `M<NNN>`, or that cites
//!   a milestone the roadmap does not define;
//! - a spec the roadmap does not mention, and a `specs/…` link in the roadmap
//!   that names no spec;
//! - a milestone row whose **State** cell (the cell after the spec link) is
//!   not the word the spec's `**Status**:` starts with — the roadmap is read
//!   for its states, so a stale cell misreports what is open.
//!
//! `concepts/` and `specs/` are local and untracked, so on a checkout without
//! them the check reports itself skipped rather than passing in silence. It is
//! therefore not part of `just ci`; `just specs` runs it.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

const ROADMAP: &str = "concepts/ROADMAP.md";
const SPECS: &str = "specs";

/// Runs the check over `root`; `false` when it found a problem.
pub fn run(root: &Path) -> bool {
    if let Some(why) = skipped(root) {
        println!("SKIPPED check-specs: {why}");
        return true;
    }
    match problems(root) {
        Ok(found) if found.is_empty() => {
            println!("check-specs: specs and roadmap agree");
            true
        }
        Ok(found) => {
            for p in &found {
                eprintln!("check-specs: {p}");
            }
            eprintln!("check-specs: {} problem(s)", found.len());
            false
        }
        Err(e) => {
            eprintln!("check-specs: {e}");
            false
        }
    }
}

/// Why the check cannot run here, if it cannot.
fn skipped(root: &Path) -> Option<String> {
    if !root.join(SPECS).is_dir() {
        return Some(format!("no {SPECS}/ directory (it is local and untracked)"));
    }
    if !root.join(ROADMAP).is_file() {
        return Some(format!("no {ROADMAP} to check the specifications against"));
    }
    None
}

/// The problems between `specs/` and the roadmap.
fn problems(root: &Path) -> std::io::Result<Vec<String>> {
    let roadmap = fs::read_to_string(root.join(ROADMAP))?;
    let defined = milestones_defined(&roadmap);
    let parked = parked_list(&roadmap);

    let mut dirs: Vec<_> = fs::read_dir(root.join(SPECS))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.join("spec.md").is_file())
        .collect();
    dirs.sort();

    let mut problems = Vec::new();
    let mut names = BTreeSet::new();
    for dir in dirs {
        let name = dir
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let body = fs::read_to_string(dir.join("spec.md"))?;
        let tasks = fs::read_to_string(dir.join("tasks.md")).unwrap_or_default();
        let stated = body
            .lines()
            .find_map(|l| l.strip_prefix("**Status**:"))
            .map_or("(none)", str::trim);

        if stated.starts_with("Parked") {
            if !parked.contains(&name) {
                problems.push(format!(
                    "specs/{name} states Status `Parked` and the roadmap's parked list does not \
                     name it — parking is a decision the roadmap records"
                ));
            }
        } else {
            match derived_status(&tasks) {
                Some(expected) if !stated.starts_with(expected) => problems.push(format!(
                    "specs/{name} states Status `{stated}` and its tasks say `{expected}`"
                )),
                Some(_) => {}
                None => problems.push(format!(
                    "specs/{name} has no task markers, so no Status can be derived from it"
                )),
            }
        }

        for artefact in ["plan.md", "tasks.md"] {
            if !dir.join(artefact).is_file() {
                problems.push(format!(
                    "specs/{name} has no {artefact}; the workflow is specify \u{2192} plan \u{2192} tasks"
                ));
            }
        }

        let cited = milestones_cited(&body);
        match own_milestone(&name) {
            Some(own) if !cited.contains(&own) => problems.push(format!(
                "specs/{name}/spec.md does not cite its own milestone M{own}"
            )),
            Some(_) => {}
            None => problems.push(format!(
                "specs/{name} does not start with a three-digit number"
            )),
        }
        for m in cited.iter().filter(|m| !defined.contains(*m)) {
            problems.push(format!(
                "specs/{name}/spec.md cites milestone M{m}, which {ROADMAP} does not define"
            ));
        }
        if !roadmap.contains(&format!("specs/{name}/")) {
            problems.push(format!("{ROADMAP} does not link specs/{name}"));
        } else if let Some(cell) = row_state(&roadmap, &name) {
            let word = STATES.iter().find(|w| stated.starts_with(**w));
            if word != Some(&cell.as_str()) {
                problems.push(format!(
                    "{ROADMAP} gives specs/{name} the state `{cell}` and its spec says `{stated}`"
                ));
            }
        } else {
            problems.push(format!(
                "{ROADMAP} has no milestone row with a State cell for specs/{name}"
            ));
        }
        names.insert(name);
    }

    for linked in specs_linked(&roadmap) {
        if !names.contains(&linked) {
            problems.push(format!(
                "{ROADMAP} links specs/{linked}, which is not a spec"
            ));
        }
    }
    Ok(problems)
}

/// The words a milestone's state can be, as a spec's `**Status**:` starts.
const STATES: [&str; 5] = ["Not started", "In progress", "Blocked", "Built", "Parked"];

/// The State cell of the milestone row linking `specs/<name>/`: the table
/// cell after the one holding the link.
fn row_state(roadmap: &str, name: &str) -> Option<String> {
    let link = format!("specs/{name}/");
    roadmap
        .lines()
        .filter(|l| l.starts_with("| **M") && l.contains(&link))
        .find_map(|l| {
            let cells: Vec<&str> = l.split('|').map(str::trim).collect();
            let at = cells.iter().position(|c| c.contains(&link))?;
            cells.get(at + 1).map(|c| (*c).to_owned())
        })
}

/// The task lines carrying `marker` — `- [x] **T001** …` — so a checkbox
/// quoted in prose is never counted as a task. A retired task — its block notes
/// `(dropped: …)` or `(superseded by …)` — is not counted at all: it keeps its
/// id so citations resolve, but it was never built, so it cannot make a
/// feature „In progress".
fn task_count(tasks: &str, marker: &str) -> usize {
    task_blocks(tasks)
        .into_iter()
        .filter(|block| !is_retired(block))
        .filter(|block| {
            block
                .trim_start()
                .strip_prefix(marker)
                .is_some_and(|rest| rest.trim_start().starts_with("**T"))
        })
        .count()
}

/// Each task with its continuation lines: a block runs from one `- [?] **T…`
/// line to the next task line, heading or blank line.
fn task_blocks(tasks: &str) -> Vec<String> {
    let is_task = |l: &str| {
        let t = l.trim_start();
        ["- [ ]", "- [x]", "- [~]"].iter().any(|m| {
            t.strip_prefix(m)
                .is_some_and(|r| r.trim_start().starts_with("**T"))
        })
    };
    let mut blocks: Vec<String> = Vec::new();
    let mut open = false;
    for line in tasks.lines() {
        if is_task(line) {
            blocks.push(line.to_owned());
            open = true;
        } else if open && !line.trim().is_empty() && !line.starts_with('#') {
            if let Some(last) = blocks.last_mut() {
                last.push('\n');
                last.push_str(line);
            }
        } else {
            open = false;
        }
    }
    blocks
}

/// A task kept only for its id: dropped as not worth building, or superseded by
/// another task that does the work.
fn is_retired(block: &str) -> bool {
    block.contains("(dropped:") || block.contains("(superseded by")
}

/// What a tasks file says about its feature: `[x]` done, `[~]` waiting on
/// somebody who is not us, `[ ]` open.
fn derived_status(tasks: &str) -> Option<&'static str> {
    // Only task lines count — `- [x] **T001** …` — so a checkbox quoted in
    // prose cannot change a feature's state.
    let count = |marker: &str| task_count(tasks, marker);
    let retired = task_blocks(tasks).iter().filter(|b| is_retired(b)).count();
    match (count("- [x]"), count("- [~]"), count("- [ ]")) {
        // Every task retired: nothing is left to build.
        (0, 0, 0) if retired > 0 => Some("Built"),
        (0, 0, 0) => None,
        (0, _, open) if open > 0 => Some("Not started"),
        (_, _, open) if open > 0 => Some("In progress"),
        // Nothing open, but something still waits on somebody else: the
        // feature is not built, whatever is already done.
        (_, waiting, _) if waiting > 0 => Some("Blocked"),
        _ => Some("Built"),
    }
}

/// The milestones the roadmap defines: a table row opening `| **M<n>**`.
fn milestones_defined(roadmap: &str) -> BTreeSet<String> {
    roadmap
        .lines()
        .filter_map(|l| l.strip_prefix("| **M"))
        .filter_map(|rest| rest.split("**").next())
        .map(|id| id.trim().to_owned())
        .collect()
}

/// The milestone a spec directory owns: `017-retail-…` owns `M17`.
fn own_milestone(name: &str) -> Option<String> {
    let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() != 3 {
        return None;
    }
    digits.parse::<u32>().ok().map(|n| n.to_string())
}

/// The spec directories the roadmap links: `specs/<name>/`.
fn specs_linked(roadmap: &str) -> BTreeSet<String> {
    roadmap
        .split("specs/")
        .skip(1)
        .filter_map(|rest| rest.split_once('/').map(|(name, _)| name))
        .filter(|name| own_milestone(name).is_some())
        .filter(|name| {
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .map(str::to_owned)
        .collect()
}

/// The roadmap's parked list: the section under a heading that says
/// „Parked", up to the next heading of the same or a higher level.
fn parked_list(roadmap: &str) -> String {
    let mut out = String::new();
    let mut level = None;
    for line in roadmap.lines() {
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if hashes > 0 && line[hashes..].starts_with(' ') {
            if level.is_some_and(|l| hashes <= l) {
                level = None;
            }
            if level.is_none() && line.to_lowercase().contains("parked") {
                level = Some(hashes);
                continue;
            }
        }
        if level.is_some() {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// The milestone ids a document cites: `M` + digits, not inside a longer word.
fn milestones_cited(text: &str) -> BTreeSet<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = BTreeSet::new();
    for (i, c) in chars.iter().enumerate() {
        if *c != 'M' || (i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) {
            continue;
        }
        let mut id = String::new();
        let mut j = i + 1;
        while j < chars.len() && chars[j].is_ascii_digit() {
            id.push(chars[j]);
            j += 1;
        }
        if id.is_empty() || (j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_')) {
            continue;
        }
        out.insert(id);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const ROADMAP_OK: &str = "# Roadmap\n\n## Milestones\n\n\
        | M | Spec | State | What |\n|---|---|---|---|\n\
        | **M1** | [`001-open`](../specs/001-open/spec.md) | Not started | open work |\n\n\
        ## Parked\n\n\
        | **M2** | [`002-later`](../specs/002-later/spec.md) | Parked | later |\n";

    fn tree(tag: &str, roadmap: &str, specs: &[(&str, &str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mako-check-specs-{tag}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("concepts")).unwrap();
        fs::write(root.join(ROADMAP), roadmap).unwrap();
        for (name, spec, tasks) in specs {
            let dir = root.join(SPECS).join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("spec.md"), spec).unwrap();
            fs::write(dir.join("plan.md"), "# Plan\n").unwrap();
            fs::write(dir.join("tasks.md"), tasks).unwrap();
        }
        root
    }

    fn open_spec() -> (&'static str, &'static str, &'static str) {
        (
            "001-open",
            "**Status**: Not started — one task\n\n**Input**: M1\n",
            "- [ ] **T001** do it\n",
        )
    }

    fn parked_spec(tasks: &'static str) -> (&'static str, &'static str, &'static str) {
        (
            "002-later",
            "**Status**: Parked — returns later\n\n**Input**: M2\n",
            tasks,
        )
    }

    #[test]
    fn status_follows_the_markers() {
        assert_eq!(derived_status(""), None);
        assert_eq!(
            derived_status("- [ ] **T001** a\n- [~] **T002** b"),
            Some("Not started")
        );
        assert_eq!(
            derived_status("- [x] **T001** a\n- [ ] **T002** b"),
            Some("In progress")
        );
        assert_eq!(derived_status("- [~] **T001** a"), Some("Blocked"));
        assert_eq!(
            derived_status("- [x] **T001** a\n- [~] **T002** b"),
            Some("Blocked")
        );
        assert_eq!(
            derived_status("- [x] **T001** a\n- [x] **T002** b"),
            Some("Built")
        );
        // A checkbox in prose is not a task.
        assert_eq!(
            derived_status("Mark done with `- [x]`.\n- [ ] **T001** a"),
            Some("Not started")
        );
    }

    /// A dropped or superseded task keeps its id but is not work done, so a
    /// spec whose only `[x]` tasks are retired has not started.
    #[test]
    fn a_retired_task_is_not_progress() {
        let tasks = "- [x] **T001** gone\n      (dropped: overengineered)\n\
                     - [x] **T002** old path\n      (superseded by T003: one jump)\n\
                     - [ ] **T003** the work\n";
        assert_eq!(derived_status(tasks), Some("Not started"));
        let built =
            "- [x] **T001** real\n      (done: path)\n- [x] **T002** x\n      (dropped: y)\n";
        assert_eq!(derived_status(built), Some("Built"));
        let only_retired = "- [x] **T001** x\n      (dropped: y)\n";
        assert_eq!(derived_status(only_retired), Some("Built"));
    }

    #[test]
    fn a_milestone_inside_a_word_is_not_cited() {
        let cited = milestones_cited("M1, (M23) and MaBiS, FM12x, Modell 2, M4a");
        assert_eq!(cited, ["1", "23"].into_iter().map(str::to_owned).collect());
    }

    #[test]
    fn an_agreeing_tree_passes() {
        let root = tree(
            "ok",
            ROADMAP_OK,
            &[open_spec(), parked_spec("- [~] **T001** waits\n")],
        );
        assert_eq!(problems(&root).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_status_the_tasks_contradict_is_refused() {
        let (name, _, tasks) = open_spec();
        let root = tree(
            "status",
            ROADMAP_OK,
            &[
                (name, "**Status**: Built — done\n\n**Input**: M1\n", tasks),
                parked_spec("- [~] **T001** w\n"),
            ],
        );
        let found = problems(&root).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains("its tasks say `Not started`")),
            "{found:?}"
        );
    }

    #[test]
    fn a_parked_spec_may_carry_open_tasks() {
        // The roadmap's parked list records the scheduling wait; `[~]` stays
        // reserved for a wait on somebody who is not us, so a parked spec
        // carries buildable work as `[ ]` beside its external waits.
        let root = tree(
            "parked-open",
            ROADMAP_OK,
            &[
                open_spec(),
                parked_spec("- [ ] **T001** buildable\n- [~] **T002** waits on a Festlegung\n"),
            ],
        );
        assert_eq!(problems(&root).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_parked_spec_outside_the_parked_list_is_refused() {
        let roadmap = ROADMAP_OK.replace("## Parked", "## Later");
        let root = tree(
            "parked-unlisted",
            &roadmap,
            &[open_spec(), parked_spec("- [~] **T001** w\n")],
        );
        let found = problems(&root).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains("parked list does not name")),
            "{found:?}"
        );
    }

    #[test]
    fn a_spec_must_cite_its_own_milestone() {
        let (name, _, tasks) = open_spec();
        let root = tree(
            "own",
            ROADMAP_OK,
            &[
                (
                    name,
                    "**Status**: Not started — x\n\n**Input**: M2\n",
                    tasks,
                ),
                parked_spec("- [~] **T001** w\n"),
            ],
        );
        let found = problems(&root).unwrap();
        assert!(
            found.iter().any(|p| p.contains("its own milestone M1")),
            "{found:?}"
        );
    }

    #[test]
    fn an_undefined_milestone_is_refused() {
        let (name, _, tasks) = open_spec();
        let root = tree(
            "undefined",
            ROADMAP_OK,
            &[
                (
                    name,
                    "**Status**: Not started — x\n\n**Input**: M1, after M9\n",
                    tasks,
                ),
                parked_spec("- [~] **T001** w\n"),
            ],
        );
        let found = problems(&root).unwrap();
        assert!(found.iter().any(|p| p.contains("M9, which")), "{found:?}");
    }

    #[test]
    fn the_roadmap_and_the_specs_name_each_other() {
        let roadmap =
            format!("{ROADMAP_OK}| **M3** | [`003-gone`](../specs/003-gone/spec.md) | x |\n");
        let root = tree("both-ways", &roadmap, &[open_spec()]);
        let found = problems(&root).unwrap();
        assert!(
            found.iter().any(|p| p.contains("links specs/003-gone")),
            "{found:?}"
        );
        assert!(
            found.iter().any(|p| p.contains("links specs/002-later")),
            "{found:?}"
        );

        let unlisted = tree(
            "unlisted",
            &ROADMAP_OK.replace("../specs/001-open/spec.md", "#"),
            &[open_spec(), parked_spec("- [~] **T001** w\n")],
        );
        let found = problems(&unlisted).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains("does not link specs/001-open")),
            "{found:?}"
        );
    }

    #[test]
    fn a_stale_state_cell_is_refused() {
        let roadmap = ROADMAP_OK.replace("| Not started |", "| Built |");
        let root = tree(
            "state-cell",
            &roadmap,
            &[open_spec(), parked_spec("- [~] **T001** w\n")],
        );
        let found = problems(&root).unwrap();
        assert!(
            found
                .iter()
                .any(|p| p.contains("gives specs/001-open the state `Built`")),
            "{found:?}"
        );
    }

    #[test]
    fn a_row_without_a_state_cell_is_refused() {
        let roadmap = ROADMAP_OK.replace(" | Not started |", " |");
        let root = tree(
            "state-missing",
            &roadmap,
            &[open_spec(), parked_spec("- [~] **T001** w\n")],
        );
        let found = problems(&root).unwrap();
        assert!(
            found.iter().any(|p| p.contains("state `open work`")),
            "{found:?}"
        );
    }

    #[test]
    fn a_checkout_without_the_local_trees_is_skipped_not_passed_silently() {
        assert!(skipped(Path::new("/nonexistent-mako-root")).is_some());
    }
}
