//! Guard: every `just ci` recipe that is one call of the xtask binary is run
//! by a job of `.github/workflows/ci.yml`.
//!
//! `just ci` is the gate a developer runs; `ci.yml` is the gate a pull request
//! meets. A guard the first runs and the second does not lets its violation
//! reach `main`, to be found by whoever next runs `just ci` — and a hand-kept
//! list of guard steps in the workflow falls behind the justfile with every
//! guard added. So the workflow derives its list: one step runs each recipe
//! `cargo xtask evidence-pack --xtask-recipes` prints (the `_ci` prerequisites
//! whose body is one xtask call), and this guard refuses a workflow in which
//! that step is missing and a recipe is run by no step of its own.
//!
//! A recipe counts as run when the workflow carries the derived step — a line
//! reading `evidence-pack --xtask-recipes` and a line running `just "$…"` —
//! or a step naming it as `just <recipe>`, or a step running the same xtask
//! arguments its body does (`cargo xtask <args>`, `cargo run -p xtask --
//! <args>`). Comment lines are not steps.

use std::path::Path;

use crate::evidence_pack::{recipe_bodies, xtask_invocation, xtask_recipes};

const WORKFLOW: &str = ".github/workflows/ci.yml";

/// The lowest number of xtask recipes a healthy justfile yields — below it the
/// recipe parser has stopped matching and a clean result is about nothing.
const MIN_RECIPES: usize = 40;

pub fn run(workspace_root: &Path) -> bool {
    let Ok(justfile) = std::fs::read_to_string(workspace_root.join("justfile")) else {
        eprintln!("check-ci-guards: cannot read justfile");
        return false;
    };
    let Ok(workflow) = std::fs::read_to_string(workspace_root.join(WORKFLOW)) else {
        eprintln!("check-ci-guards: cannot read {WORKFLOW}");
        return false;
    };
    let recipes = xtask_recipes(&justfile);
    if recipes.len() < MIN_RECIPES {
        eprintln!(
            "check-ci-guards: found {} xtask recipe(s) among the `_ci` prerequisites; at \
             least {MIN_RECIPES} are expected. The parser has stopped matching — fix it \
             before trusting a green run.",
            recipes.len()
        );
        return false;
    }
    let missing = unrun(&justfile, &workflow);
    if missing.is_empty() {
        println!(
            "check-ci-guards: all {} xtask recipes of `just ci` run in {WORKFLOW}",
            recipes.len()
        );
        return true;
    }
    eprintln!(
        "check-ci-guards: {} `just ci` recipe(s) no {WORKFLOW} job runs — run them from the \
         step that loops over `cargo xtask evidence-pack --xtask-recipes`:",
        missing.len()
    );
    for r in &missing {
        eprintln!("  {r}");
    }
    false
}

/// The workflow's lines that are not comments.
fn step_lines(workflow: &str) -> Vec<&str> {
    workflow
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// `needle` occurs in `line` followed by the end of the line or whitespace.
fn names(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(i, _)| {
        line[i + needle.len()..]
            .chars()
            .next()
            .is_none_or(char::is_whitespace)
    })
}

/// The workflow carries the step that runs every recipe the justfile lists.
pub fn derives_the_list(workflow: &str) -> bool {
    let lines = step_lines(workflow);
    lines
        .iter()
        .any(|l| l.contains("evidence-pack --xtask-recipes"))
        && lines.iter().any(|l| l.contains("just \"$"))
}

/// The `_ci` xtask recipes `workflow` runs in no step, in `_ci` order.
pub fn unrun(justfile: &str, workflow: &str) -> Vec<String> {
    let recipes = xtask_recipes(justfile);
    if derives_the_list(workflow) {
        return Vec::new();
    }
    let bodies = recipe_bodies(justfile);
    let lines = step_lines(workflow);
    recipes
        .into_iter()
        .filter(|r| {
            let args = bodies
                .get(r)
                .and_then(|b| b.first())
                .and_then(|l| xtask_invocation(l));
            !lines.iter().any(|l| {
                names(l, &format!("just {r}"))
                    || args.is_some_and(|a| {
                        names(l, &format!("xtask {a}")) || names(l, &format!("xtask -- {a}"))
                    })
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const JUSTFILE: &str = "\
_ci: check check-a check-b check-c regulatories

check:
    cargo check --workspace

check-a:
    cargo xtask check-a

check-b:
    cargo run -q -p xtask -- check-b

check-c:
    cargo xtask check-c

regulatories:
    cargo xtask sync-regulatories --offline
";

    #[test]
    fn a_hand_listed_workflow_names_the_recipes_it_leaves_out() {
        let wf = "\
jobs:
  xtask:
    steps:
      - run: cargo run -p xtask -- check-b
      # - run: cargo xtask check-c
      - run: cargo xtask check-ab
      - run: cargo xtask sync-regulatories --offline
";
        assert_eq!(unrun(JUSTFILE, wf), ["check-a", "check-c"]);
    }

    #[test]
    fn a_just_step_and_an_xtask_step_both_count() {
        let wf = "      - run: just check-a\n      - run: cargo xtask check-c\n\
                  - run: cargo xtask check-b\n      - run: just regulatories\n";
        assert!(unrun(JUSTFILE, wf).is_empty());
    }

    #[test]
    fn the_derived_step_runs_every_recipe() {
        let wf = "\
      - run: |
          for r in $(cargo xtask evidence-pack --xtask-recipes); do
            just \"$r\"
          done
";
        assert!(derives_the_list(wf));
        assert!(unrun(JUSTFILE, wf).is_empty());
    }

    #[test]
    fn a_commented_out_derived_step_is_not_a_step() {
        let wf = "      # for r in $(cargo xtask evidence-pack --xtask-recipes); do\n\
                  #   just \"$r\"\n";
        assert!(!derives_the_list(wf));
        assert_eq!(unrun(JUSTFILE, wf).len(), 4);
    }

    #[test]
    fn the_real_workflow_runs_every_xtask_recipe_of_the_real_justfile() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let justfile = std::fs::read_to_string(root.join("justfile")).unwrap();
        let workflow = std::fs::read_to_string(root.join(WORKFLOW)).unwrap();
        assert!(xtask_recipes(&justfile).len() >= MIN_RECIPES);
        assert_eq!(unrun(&justfile, &workflow), Vec::<String>::new());
    }
}
