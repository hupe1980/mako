//! Guard: every command a service dispatches to `makod` is one `makod` has.
//!
//! `makod` refuses an unknown command name with `422`, so a caller that spells
//! the wire name a second time can drift from the registry and only find out
//! when a real message arrives: the work is done, the dispatch fails, and the
//! Frist expires on a process that looked healthy.
//!
//! `mako_markt::commands` exists so the name is written once. Three rules keep
//! callers inside it, applied to **every** service's `src/` tree — `processd`
//! alone dispatches around forty commands, and a typo in any of them 422s at
//! run time with every check still green:
//!
//! 1. **No bare literals.** A string literal shaped like a command name
//!    (`<prozess>.<vorgang>.<verb>`) in a service's shipped code is refused,
//!    whether or not `makod` happens to register it. A literal that resolves is
//!    outside the mechanism; a literal that does not is the 422 itself.
//! 2. **Every constant resolves** to a name `makod` registers.
//! 3. **Every constant is in `DISPATCHED_BY_SERVICES`** — one that is missing
//!    is one `makod`'s own registry test never looks at.
//!
//! `services/makod/` is excluded: it is the registry side of the contract, and
//! the names it holds are the ones the rules are measured against.
//!
//! Unit-test modules are excluded from rule 1. A test that pins the wire
//! spelling a mapping function returns has to *write* that spelling, and
//! `processd`'s `answer_commands` tests are exactly that assertion.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The process families a command name starts with.
const FAMILIES: &[&str] = &[
    "gpke",
    "geli",
    "wim",
    "mabis",
    "gabi",
    "invoic",
    "esa",
    "redispatch",
];

/// Command-shaped literals that name something other than a `makod` command,
/// with what they are.
///
/// The shape `<family>.<subject>.<verb>` is not reserved: `agentd` keys a skill
/// capability the same way and `sperrd` keys an obligation the same way. Neither
/// is posted to `POST /api/v1/commands`, so neither has — or should have — a
/// constant in `mako_markt::commands`.
const NOT_A_COMMAND: &[(&str, &str)] = &[
    (
        "gabi.gas.balancing",
        "`agentd`'s GaBi skill capability key, matched against a specialist \
         manifest's declared capabilities",
    ),
    (
        "gabi.final-allocation.missing",
        "an `agentd` triage finding key, carried in the skill's own output",
    ),
    (
        "gpke.sperrauftrag.termingebunden",
        "`sperrd`'s obligation key for a date-bound Sperrauftrag, stored on the \
         order row and read back by the executor",
    ),
];

/// Every `name: "…"` in `makod`'s command registry.
fn makod_commands(workspace_root: &Path) -> BTreeSet<String> {
    let path = workspace_root.join("services/makod/src/orchestrator/commands_api/registry.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    src.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("name: \"")?;
            let end = rest.find('"')?;
            Some(rest[..end].to_owned())
        })
        .collect()
}

/// `CONST_NAME` → wire name, from the shared catalogue.
///
/// Read over the whole file rather than line by line: `rustfmt` puts the value
/// of a long constant on its own line, and a reader that only looked at the
/// `pub const` line would silently drop those — taking rules 2 and 3 with them
/// for exactly the longest, most easily mistyped names.
fn command_constants(workspace_root: &Path) -> BTreeMap<String, String> {
    let path = workspace_root.join("crates/mako-markt/src/commands.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for (name, value) in str_constants(&src) {
        out.insert(name, value);
    }
    out
}

/// Every `pub const NAME: &str = "value";` in `src`, however it is wrapped.
///
/// The declaration must *start* a line: a `pub const` quoted inside a doc
/// comment or a test fixture is prose, not an export.
#[must_use]
pub fn str_constants(src: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(':') else {
            continue;
        };
        if !tail.trim_start().starts_with("&str") {
            continue;
        }
        // `rustfmt` puts the value of a long constant on the next line, so the
        // statement runs to its terminating `;`.
        let mut statement = tail.to_owned();
        let mut j = i;
        while !statement.contains(';') && j + 1 < lines.len() {
            j += 1;
            statement.push_str(lines[j]);
        }
        let statement = statement.split(';').next().unwrap_or_default().to_owned();
        let Some(open) = statement.find('"') else {
            continue;
        };
        let Some(close) = statement[open + 1..].find('"') else {
            continue;
        };
        out.push((
            name.trim().to_owned(),
            statement[open + 1..open + 1 + close].to_owned(),
        ));
    }
    out
}

/// Every `pub const` name the catalogue exports, command or not.
///
/// `DISPATCHED_BY_SERVICES` is one of them, and a service naming it is reading
/// the list rather than dispatching a command — so rule 2 must be able to tell
/// "not a command constant" from "no such constant".
fn exported_consts(workspace_root: &Path) -> BTreeSet<String> {
    let path = workspace_root.join("crates/mako-markt/src/commands.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    src.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("pub const ")?;
            let (name, _) = rest.split_once(':')?;
            Some(name.trim().to_owned())
        })
        .collect()
}

/// The constant names listed in `DISPATCHED_BY_SERVICES`.
fn dispatched_list(workspace_root: &Path) -> BTreeSet<String> {
    let path = workspace_root.join("crates/mako-markt/src/commands.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    let Some(start) = src.find("pub const DISPATCHED_BY_SERVICES") else {
        return BTreeSet::new();
    };
    // `= &[` — not the `[` of the `&[&str]` type ahead of it.
    let Some(open) = src[start..].find("= &[") else {
        return BTreeSet::new();
    };
    let open = open + 3;
    let Some(close) = src[start + open..].find(']') else {
        return BTreeSet::new();
    };
    src[start + open + 1..start + open + close]
        .split(',')
        .map(|e| e.trim().to_owned())
        .filter(|e| !e.is_empty())
        .collect()
}

/// Every `.rs` file under `services/*/src/`, except `makod`'s.
fn service_sources(workspace_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(services) = std::fs::read_dir(workspace_root.join("services")) else {
        return out;
    };
    let mut roots: Vec<PathBuf> = services
        .flatten()
        .map(|e| e.path())
        .filter(|p| !p.ends_with("makod"))
        .map(|p| p.join("src"))
        .filter(|p| p.is_dir())
        .collect();
    roots.sort();
    for root in roots {
        collect_rs(&root, &mut out);
    }
    out.sort();
    out
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A constant named through the shared catalogue, as `(1-based line, name)`.
#[must_use]
pub fn catalogue_uses(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        let line = strip_comment(raw);
        let mut rest = line.as_str();
        while let Some(at) = rest.find("commands::") {
            let tail = &rest[at + "commands::".len()..];
            let name: String = tail
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.push((i + 1, name));
            }
            rest = &rest[at + "commands::".len()..];
        }
    }
    out
}

/// Command-shaped string literals written out by hand, as `(1-based line, text)`.
///
/// Comments and `#[cfg(test)]` modules are skipped: prose about a command names
/// it, and a test pinning the spelling a function returns has to write it.
#[must_use]
pub fn bare_literals(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut test_mod_depth: Option<i32> = None;
    let mut recent: Vec<&str> = Vec::new();

    for (i, raw) in src.lines().enumerate() {
        let line = strip_comment(raw);
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;

        if test_mod_depth.is_none()
            && line.contains("mod ")
            && line.contains('{')
            && recent.iter().any(|l| l.trim_start() == "#[cfg(test)]")
        {
            test_mod_depth = Some(depth);
        }
        depth += opens - closes;
        if let Some(base) = test_mod_depth
            && depth <= base
        {
            test_mod_depth = None;
        } else if test_mod_depth.is_some() {
            recent.push(raw);
            if recent.len() > 3 {
                recent.remove(0);
            }
            continue;
        }

        for literal in string_literals(&line) {
            if is_command_shaped(&literal) && !NOT_A_COMMAND.iter().any(|(l, _)| *l == literal) {
                out.push((i + 1, literal));
            }
        }
        recent.push(raw);
        if recent.len() > 3 {
            recent.remove(0);
        }
    }
    out
}

/// `<family>.<subject>.<verb>`, the shape every `makod` command name has.
#[must_use]
pub fn is_command_shaped(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 3 || !FAMILIES.contains(&parts[0]) {
        return false;
    }
    parts[1..].iter().all(|p| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

/// Every string literal on one line, without its quotes.
///
/// A literal left open at the end of the line — a `\`-continued or multi-line
/// string — yields the rest of the line, which no command shape survives.
fn string_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut escaped = false;
    for c in line.chars() {
        match current.as_mut() {
            Some(buf) => {
                if escaped {
                    escaped = false;
                    buf.push(c);
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    out.push(std::mem::take(buf));
                    current = None;
                } else {
                    buf.push(c);
                }
            }
            None if c == '"' => current = Some(String::new()),
            None => {}
        }
    }
    if let Some(buf) = current {
        out.push(buf);
    }
    out
}

/// Strip a trailing `//` comment, ignoring one inside a string literal.
fn strip_comment(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && bytes.get(i + 1) == Some(&b'/') => return line[..i].to_owned(),
            _ => {}
        }
        i += 1;
    }
    line.to_owned()
}

/// Check every dispatched command against the catalogue and the registry.
///
/// Returns `true` when all three rules hold.
pub fn run(workspace_root: &Path) -> bool {
    let registry = makod_commands(workspace_root);
    let constants = command_constants(workspace_root);
    let exported = exported_consts(workspace_root);
    let dispatched = dispatched_list(workspace_root);
    let sources = service_sources(workspace_root);

    let mut uses: Vec<(String, usize, String)> = Vec::new();
    let mut literals: Vec<(String, usize, String)> = Vec::new();
    for path in &sources {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for (line, name) in catalogue_uses(&src) {
            uses.push((rel.clone(), line, name));
        }
        for (line, text) in bare_literals(&src) {
            literals.push((rel.clone(), line, text));
        }
    }

    if registry.len() < 50 || constants.len() < 30 || uses.len() < 50 {
        eprintln!(
            "check-answer-commands: scanned {} registry entries, {} constants and {} \
             catalogue uses across {} service source files — has the layout changed?",
            registry.len(),
            constants.len(),
            uses.len(),
            sources.len()
        );
        return false;
    }

    let mut ok = true;
    for (rel, line, text) in &literals {
        eprintln!(
            "check-answer-commands: {rel}:{line} dispatches the bare literal {text:?}.\n  \
             Name the constant in `mako_markt::commands` instead: a literal is outside the \
             mechanism that keeps the wire name and makod's registry linked."
        );
        ok = false;
    }
    for (rel, line, const_name) in &uses {
        let Some(wire) = constants.get(const_name) else {
            // Another export of the module — the `DISPATCHED_BY_SERVICES` list
            // itself — is being read, not a command being named.
            if exported.contains(const_name) {
                continue;
            }
            eprintln!(
                "check-answer-commands: {rel}:{line} names `commands::{const_name}`, which \
                 `mako_markt::commands` does not define."
            );
            ok = false;
            continue;
        };
        if !registry.contains(wire) {
            eprintln!(
                "check-answer-commands: {rel}:{line} dispatches {wire:?} (`{const_name}`), \
                 which makod does not register.\n  The check would run, the verdict would be \
                 persisted, and the dispatch would fail — so the answer never reaches the \
                 counterparty and the Antwortfrist expires on a process that looked healthy."
            );
            ok = false;
        }
        if !dispatched.contains(const_name) {
            eprintln!(
                "check-answer-commands: `{const_name}` is dispatched by {rel} but is missing \
                 from `DISPATCHED_BY_SERVICES`, so makod's own registry test never checks it."
            );
            ok = false;
        }
    }

    if ok {
        println!(
            "check-answer-commands: {} catalogue uses across {} service source files all name \
             a constant that makod registers, and no service spells a command by hand",
            uses.len(),
            sources.len()
        );
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::{bare_literals, catalogue_uses, is_command_shaped, str_constants};

    #[test]
    fn a_command_shape_is_three_lowercase_parts() {
        assert!(is_command_shaped("gpke.lieferbeginn.bestaetigen"));
        assert!(is_command_shaped("wim.wertebestellung.anfrage-ablehnen"));
        assert!(!is_command_shaped("gpke.lieferbeginn"));
        assert!(!is_command_shaped("erp.lieferbeginn.bestaetigen"));
        assert!(!is_command_shaped("gpke.Lieferbeginn.bestaetigen"));
    }

    /// `processd` dispatches around forty commands and none of them went
    /// through this check while it read one file. A bare literal in any
    /// service's shipped code is the drift the constants exist to prevent.
    #[test]
    fn a_bare_literal_outside_invoicd_is_caught() {
        let src = "fn answer(pid: u32) -> &'static str {\n\
                   \x20   match pid {\n\
                   \x20       55_001 => \"gpke.lieferbeginn.bestaetigen\",\n\
                   \x20       _ => \"gpke.lieferende.ablehnen\",\n\
                   \x20   }\n\
                   }\n";
        let hits = bare_literals(src);
        assert_eq!(hits.len(), 2, "{hits:?}");
    }

    /// A test pinning the spelling a mapping function returns has to write it.
    #[test]
    fn a_unit_test_module_may_write_the_spelling() {
        let src = "#[cfg(test)]\n\
                   mod tests {\n\
                   \x20   #[test]\n\
                   \x20   fn names() {\n\
                   \x20       assert_eq!(answer(55_001), \"gpke.lieferbeginn.bestaetigen\");\n\
                   \x20   }\n\
                   }\n\
                   fn shipped() -> &'static str {\n\
                   \x20   \"gpke.lieferende.ablehnen\"\n\
                   }\n";
        let hits = bare_literals(src);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].1, "gpke.lieferende.ablehnen");
    }

    /// An obligation key and a skill capability share the shape and are not
    /// commands; prose naming a command is prose.
    #[test]
    fn the_documented_non_commands_are_not_findings() {
        let src = "const CAPABILITY: &str = \"gabi.gas.balancing\";\n\
                   let key = \"gpke.sperrauftrag.termingebunden\";\n\
                   // dispatch \"gpke.lieferbeginn.anmelden\" here\n";
        assert!(bare_literals(src).is_empty(), "{:?}", bare_literals(src));
    }

    /// `rustfmt` wraps a long constant, and the value lands on the next line.
    #[test]
    fn a_wrapped_constant_still_resolves() {
        let src = concat!(
            "pub const SHORT: &str = \"gpke.a.b\";\n",
            "pub const LONG_ENOUGH_TO_WRAP: &str =\n",
            "    \"wim.wertebestellung.bestellung-beantworten\";\n",
            "pub const LIST: &[&str] = &[SHORT];\n",
        );
        assert_eq!(
            str_constants(src),
            vec![
                ("SHORT".to_owned(), "gpke.a.b".to_owned()),
                (
                    "LONG_ENOUGH_TO_WRAP".to_owned(),
                    "wim.wertebestellung.bestellung-beantworten".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn catalogue_uses_are_read_by_name() {
        let src = "accept: commands::GPKE_ABRECHNUNG_ANNEHMEN,\n\
                   for n in mako_markt::commands::DISPATCHED_BY_SERVICES {}\n";
        assert_eq!(
            catalogue_uses(src),
            vec![
                (1, "GPKE_ABRECHNUNG_ANNEHMEN".to_owned()),
                (2, "DISPATCHED_BY_SERVICES".to_owned()),
            ]
        );
    }
}
