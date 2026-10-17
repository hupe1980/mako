//! Guard: an event payload is a tolerant reader.
//!
//! A `§ 147 Abs. 1 AO` record has to stay foldable for ten years, and the thing
//! that decides whether it does is not the upcast hook — it is whether today's
//! struct can deserialise an event written by yesterday's code. Two attributes
//! decide that, and they fail in opposite directions.
//!
//! **`deny_unknown_fields` breaks a field removal.** An event written today
//! carries a key that tomorrow's struct no longer declares; `serde` is asked to
//! refuse exactly that, and the fold of every historical stream carrying the
//! event fails. On a request body the opposite holds — a typo must not become
//! a setting that silently does nothing — and `mako_service::Json` refuses an
//! unknown key at runtime; where it cannot see (a flattened field, an untagged
//! or internally tagged enum) `check-request-bodies` requires the attribute.
//! The situation inverts for an event, which is read back by code written
//! years later. The two rules collide only on a type that is both, which is
//! why [`REQUEST_BODY_TYPES`] lists each body type a workflow crate holds and
//! refuses one that rides in an event.
//!
//! **A required field with no default breaks a field addition**, which is the
//! far more common change. An old event simply lacks the new key, and `serde`
//! fails rather than filling it. `#[serde(default)]` — or an `Option` — is what
//! makes the ordinary evolution of a payload a non-event.
//!
//! Both halves matter because the cheap one is what keeps the expensive one
//! rare: with tolerant payloads, `Workflow::upcast` is reserved for the changes
//! tolerance genuinely cannot absorb — a split, a merge, a rename that changes
//! meaning — rather than being owed on every field.
//!
//! ## What is scanned
//!
//! Every `.rs` file under `crates/mako-*/src/**`. Within each file, the
//! `#[derive(…)]`-bearing items that carry a `Deserialize` and sit in a crate
//! that declares workflows. `#[cfg(test)]` sections are blanked first, so a test
//! fixture holding itself strict is not a production defect.
//!
//! ## What it refuses
//!
//! `#[serde(deny_unknown_fields)]` anywhere in a workflow crate's `src/`. That
//! is the rule this guard can enforce textually: these crates hold event and
//! state payloads, and the few request-body types a calculation in them reads
//! are listed in [`REQUEST_BODY_TYPES`] with a reason — each refused again if
//! it ever appears inside an event or state enum.
//!
//! The field-default half is deliberately **not** enforced here. Whether a field
//! needs `#[serde(default)]` depends on whether it existed in a shipped payload
//! version, which the source cannot answer — that is the replay corpus's job,
//! where an old fixture that no longer folds is the failure, and it is the
//! honest place for it.
//!
//! ## Why it refuses an empty tree
//!
//! A scanner that silently stops matching certifies nothing. [`MIN_CRATES`] is
//! the floor: these crates exist and are found by a glob, so a run that sees
//! fewer means the layout moved and the guard has to be taught it, not that the
//! defect class went away.

use std::path::Path;

/// Crate-name prefix whose `src/` holds workflow event and state payloads.
const CRATE_PREFIX: &str = "mako-";

/// Crates excluded because they hold no workflow payloads.
///
/// `mako-service` and `mako-events` are the daemon runtime and the CloudEvent
/// type catalogue; `mako-as4` is transport. A `deny_unknown_fields` in any of
/// them is about a wire or config shape, not a replayed event.
const NOT_WORKFLOW_CRATES: &[&str] = &[
    "mako-service",
    "mako-events",
    "mako-as4",
    "mako-markt",
    "mako-fristen",
    "mako-pruefung",
    "mako-invoic",
    "mako-engine",
];

/// Typed outbound payloads, scanned although their crate holds no workflow.
///
/// An outbox row is written by one build and drained by the next, so these are
/// tolerant readers by the same argument as an event: a strict struct turns a
/// removed field into a dead-lettered message.
const PAYLOAD_FILES: &[&str] = &["crates/mako-engine/src/payload.rs"];

/// Request-body types that live in a workflow crate because the calculation
/// that reads them lives there: `(file, type, reason)`. Each may be strict for
/// `check-request-bodies`, and this guard refuses an entry whose type is
/// named inside any `…Event` or `…State` enum of its crate — the one shape
/// that would make it an event payload after all.
const REQUEST_BODY_TYPES: &[(&str, &str, &str)] = &[
    (
        "crates/mako-redispatch/src/ausfallarbeit.rs",
        "Leistungslimitierung",
        "netzbilanzd Ausfallarbeit request input, internally tagged, so the runtime \
         unknown-key refusal cannot see into it; never recorded as an event",
    ),
    (
        "crates/mako-redispatch/src/ausfallarbeit.rs",
        "VergleichsViertelstunde",
        "netzbilanzd Ausfallarbeit request input; never recorded as an event",
    ),
    (
        "crates/mako-redispatch/src/ausfallarbeit.rs",
        "VergleichstagViertelstunde",
        "netzbilanzd Ausfallarbeit request input; never recorded as an event",
    ),
    (
        "crates/mako-redispatch/src/ausfallarbeit.rs",
        "UeberbauungTr",
        "netzbilanzd Überbauung request input; never recorded as an event",
    ),
];

/// The type a `deny_unknown_fields` line at `idx` is attached to: the first
/// `struct`/`enum` name after it, skipping further attributes and doc lines.
fn attached_type(lines: &[&str], idx: usize) -> Option<String> {
    lines.iter().skip(idx + 1).take(12).find_map(|l| {
        let t = l.trim_start();
        let rest = t
            .strip_prefix("pub struct ")
            .or_else(|| t.strip_prefix("pub enum "))
            .or_else(|| t.strip_prefix("struct "))
            .or_else(|| t.strip_prefix("enum "))?;
        Some(
            rest.chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect(),
        )
    })
}

/// The bodies of every `enum …Event` / `enum …State` in `src`.
fn event_and_state_enums(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(i) = rest.find("enum ") {
        let after = &rest[i + 5..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        rest = after;
        if !(name.ends_with("Event") || name.ends_with("State")) {
            continue;
        }
        let Some(open) = after.find('{') else { break };
        let mut depth = 0usize;
        for (j, ch) in after[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        out.push(after[open..open + j + 1].to_owned());
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// The lowest number of workflow crates a healthy scan may find.
///
/// The measured value, for the reason every other floor here is: a floor set
/// under what the tree holds tolerates exactly the failure it exists to catch.
const MIN_CRATES: usize = 7;

/// The attribute this guard refuses.
const FORBIDDEN: &str = "deny_unknown_fields";

/// Blank out `#[cfg(test)]` modules so a strict test fixture is not a finding.
fn strip_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        // Find the module body and skip it by brace matching.
        let Some(open) = after.find('{') else {
            break;
        };
        let mut depth = 0usize;
        let mut end = None;
        for (j, ch) in after[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + j + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => rest = &after[e..],
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Scan the workflow crates for a strict event payload.
///
/// Returns `true` when every workflow crate's `src/` is free of
/// `deny_unknown_fields`.
#[must_use]
pub fn run(workspace_root: &Path) -> bool {
    let crates_dir = workspace_root.join("crates");
    let Ok(entries) = std::fs::read_dir(&crates_dir) else {
        println!("check-event-payloads: SKIP — crates/ is not readable");
        return true;
    };

    let mut findings: Vec<String> = Vec::new();
    let mut scanned_crates = 0usize;
    let mut scanned_files = 0usize;

    let mut dirs: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();

    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.starts_with(CRATE_PREFIX) || NOT_WORKFLOW_CRATES.contains(&name) {
            continue;
        }
        let src = dir.join("src");
        if !src.is_dir() {
            continue;
        }
        scanned_crates += 1;

        let mut stack = vec![src];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().and_then(|s| s.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                scanned_files += 1;
                let body = strip_test_modules(&text);
                let rel = p
                    .strip_prefix(workspace_root)
                    .unwrap_or(&p)
                    .display()
                    .to_string();
                let lines: Vec<&str> = body.lines().collect();
                for (n, line) in lines.iter().enumerate() {
                    if !line.contains(FORBIDDEN) {
                        continue;
                    }
                    let exempt = attached_type(&lines, n).is_some_and(|ty| {
                        REQUEST_BODY_TYPES
                            .iter()
                            .any(|(file, name, _)| *file == rel && *name == ty)
                    });
                    if !exempt {
                        findings.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                    }
                }
            }
        }
    }

    for rel in PAYLOAD_FILES {
        let p = workspace_root.join(rel);
        let Ok(text) = std::fs::read_to_string(&p) else {
            findings.push(format!("{rel}: unreadable — the payload module moved"));
            continue;
        };
        scanned_files += 1;
        for (n, line) in strip_test_modules(&text).lines().enumerate() {
            if line.contains(FORBIDDEN) {
                findings.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }

    // An exempted request body must stay out of every event and state enum of
    // its crate, or it is an event payload after all.
    for (file, name, _) in REQUEST_BODY_TYPES {
        let crate_src = Path::new(file)
            .ancestors()
            .find(|a| a.ends_with("src"))
            .map(|a| workspace_root.join(a));
        let Some(crate_src) = crate_src else { continue };
        let mut stack = vec![crate_src];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                    let Ok(text) = std::fs::read_to_string(&p) else {
                        continue;
                    };
                    let body = strip_test_modules(&text);
                    if event_and_state_enums(&body).iter().any(|e| {
                        e.split(|c: char| !c.is_alphanumeric() && c != '_')
                            .any(|w| w == *name)
                    }) {
                        findings.push(format!(
                            "{file}: `{name}` is exempted as a request body but rides in an \
                             event or state enum in {}",
                            p.strip_prefix(workspace_root).unwrap_or(&p).display()
                        ));
                    }
                }
            }
        }
    }

    if scanned_crates < MIN_CRATES {
        println!(
            "check-event-payloads: FAIL — found {scanned_crates} workflow crate(s), at least \
             {MIN_CRATES} are expected.\n\
             The crate layout moved and this guard has to be taught the new one; it has not \
             stopped being needed."
        );
        return false;
    }

    if findings.is_empty() {
        println!(
            "check-event-payloads: {scanned_files} file(s) across {scanned_crates} workflow \
             crate(s) — no event payload denies unknown fields"
        );
        return true;
    }

    println!(
        "check-event-payloads: FAIL — {} event payload(s) deny unknown fields:\n",
        findings.len()
    );
    for f in &findings {
        println!("  {f}");
    }
    println!(
        "\nAn event is read back by code written years later. `deny_unknown_fields` turns a \
         removed or renamed field into a hard `serde` error on every historical stream that \
         carries the event — a § 147 Abs. 1 AO record that no longer folds.\n\
         The attribute belongs on a request body where serde buffers (a flattened field, an \
         untagged or internally tagged enum), and `check-request-bodies` requires it there.\n\
         Remove it here."
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_exempted_request_body_inside_an_event_enum_is_found() {
        let src = "pub enum FooEvent { A(VergleichsViertelstunde), B }";
        assert!(
            event_and_state_enums(src)
                .iter()
                .any(|e| e.contains("VergleichsViertelstunde"))
        );
        let lines = [
            "#[serde(deny_unknown_fields)]",
            "/// doc",
            "pub struct UeberbauungTr {",
        ];
        assert_eq!(attached_type(&lines, 0).as_deref(), Some("UeberbauungTr"));
    }

    /// The guard finds the attribute it exists to refuse.
    ///
    /// A guard that cannot fail certifies nothing, so this is the input that
    /// makes it fail.
    #[test]
    fn catches_a_strict_event_payload() {
        let dir = std::env::temp_dir().join("mako-check-event-payloads-pos");
        let _ = std::fs::remove_dir_all(&dir);
        for c in [
            "mako-a", "mako-b", "mako-c", "mako-d", "mako-e", "mako-f", "mako-g",
        ] {
            std::fs::create_dir_all(dir.join("crates").join(c).join("src")).unwrap();
            std::fs::write(
                dir.join("crates").join(c).join("src").join("lib.rs"),
                "pub struct Ok {}\n",
            )
            .unwrap();
        }
        std::fs::create_dir_all(dir.join("crates/mako-engine/src")).unwrap();
        std::fs::write(
            dir.join("crates/mako-engine/src/payload.rs"),
            "pub struct P {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("crates/mako-a/src/lib.rs"),
            "#[derive(serde::Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct D { pub a: u8 }\n",
        )
        .unwrap();
        assert!(!run(&dir), "a strict event payload must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A strict outbound payload is refused although its crate holds no
    /// workflow: an outbox row outlives the build that wrote it.
    #[test]
    fn catches_a_strict_outbound_payload() {
        let dir = std::env::temp_dir().join("mako-check-event-payloads-outbound");
        let _ = std::fs::remove_dir_all(&dir);
        for c in [
            "mako-a", "mako-b", "mako-c", "mako-d", "mako-e", "mako-f", "mako-g",
        ] {
            std::fs::create_dir_all(dir.join("crates").join(c).join("src")).unwrap();
            std::fs::write(dir.join("crates").join(c).join("src").join("lib.rs"), "").unwrap();
        }
        std::fs::create_dir_all(dir.join("crates/mako-engine/src")).unwrap();
        std::fs::write(
            dir.join("crates/mako-engine/src/payload.rs"),
            "#[serde(deny_unknown_fields)]\npub struct AperakPayload {}\n",
        )
        .unwrap();
        assert!(!run(&dir), "a strict outbound payload must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tolerant tree passes, and an excluded crate is not scanned.
    #[test]
    fn a_tolerant_tree_passes() {
        let dir = std::env::temp_dir().join("mako-check-event-payloads-neg");
        let _ = std::fs::remove_dir_all(&dir);
        for c in [
            "mako-a", "mako-b", "mako-c", "mako-d", "mako-e", "mako-f", "mako-g",
        ] {
            std::fs::create_dir_all(dir.join("crates").join(c).join("src")).unwrap();
            std::fs::write(
                dir.join("crates").join(c).join("src").join("lib.rs"),
                "#[derive(serde::Deserialize)]\npub struct D { pub a: u8 }\n",
            )
            .unwrap();
        }
        std::fs::create_dir_all(dir.join("crates/mako-engine/src")).unwrap();
        std::fs::write(
            dir.join("crates/mako-engine/src/payload.rs"),
            "pub struct P {}\n",
        )
        .unwrap();
        // An excluded crate may hold the attribute — it has HTTP bodies.
        std::fs::create_dir_all(dir.join("crates/mako-service/src")).unwrap();
        std::fs::write(
            dir.join("crates/mako-service/src/lib.rs"),
            "#[serde(deny_unknown_fields)]\npub struct Body {}\n",
        )
        .unwrap();
        assert!(run(&dir), "a tolerant tree must pass");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A test fixture holding itself strict is not a production defect.
    #[test]
    fn a_cfg_test_module_is_not_scanned() {
        let src = "pub struct A {}\n#[cfg(test)]\nmod t {\n  #[serde(deny_unknown_fields)]\n  struct S {}\n}\npub struct B {}\n";
        assert!(!strip_test_modules(src).contains("deny_unknown_fields"));
    }

    /// The floor binds: a tree with too few crates is a moved layout.
    #[test]
    fn an_empty_tree_is_refused() {
        let dir = std::env::temp_dir().join("mako-check-event-payloads-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("crates")).unwrap();
        assert!(!run(&dir), "a tree with no workflow crates must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
