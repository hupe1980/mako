//! Guard: an event that is a market message says so.
//!
//! The Desk's Verlauf shows a process's messages — which arrived, which were
//! sent, with their PID — from what the engine stamped on each envelope at
//! append ([`EventPayload::verlauf`]). An event type's name is an identifier,
//! not a vocabulary a reader may parse, so a message whose event states
//! nothing is missing from every history: the operator sees the process jump
//! from one state to the next with no message between.
//!
//! The textual rule: in a `crates/*/src` `impl EventPayload for T` block, if an
//! `event_type` arm names a message — `…Received`, `…Empfangen`, `…Gesendet`,
//! `…Dispatched`, `…Sent`, `…Eingegangen`, `…Versendet`, `…Erhalten`,
//! `…Uebermittelt`, `…Geliefert`, `…Beantwortet`, `…Gemeldet` — the block defines
//! `fn verlauf`. A type whose variants are no messages at all needs nothing;
//! one that is a message under a name the rule does not read (`Initiated`) is
//! the author's to state, and the tests beside each impl pin it.
//!
//! [`EventPayload::verlauf`]: https://docs.rs/mako-engine

use std::path::Path;

const MESSAGE_SUFFIXES: &[&str] = &[
    "Received",
    "Empfangen",
    "Gesendet",
    "Dispatched",
    "Sent",
    "Eingegangen",
    "Versendet",
    "Erhalten",
    "Uebermittelt",
    "Geliefert",
    "Beantwortet",
    "Gemeldet",
];

/// `(type, names a message, defines verlauf)` for each `impl EventPayload`.
fn impls(src: &str) -> Vec<(String, bool, bool)> {
    let mut out = Vec::new();
    let mut rest = src;
    while let Some(at) = rest.find("EventPayload for ") {
        let after = &rest[at + "EventPayload for ".len()..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let Some(open) = after.find('{') else { break };
        let mut depth = 0usize;
        let mut end = open;
        for (i, c) in after[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let block = &after[open..end];
        if name.is_empty() {
            // A macro's `impl EventPayload for $t`: its expansions are out of a
            // textual check's sight. mako-redispatch's `ack_forward` macro
            // forwards `verlauf` to the event it wraps, pinned by its test.
            rest = &after[end..];
            continue;
        }
        let names_message = block.split('"').skip(1).step_by(2).any(|lit| {
            lit.chars().next().is_some_and(char::is_uppercase)
                && MESSAGE_SUFFIXES.iter().any(|s| lit.ends_with(s))
        });
        out.push((name, names_message, block.contains("fn verlauf")));
        rest = &after[end..];
    }
    out
}

fn collect(dir: &Path, root: &Path, files: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            collect(&p, root, files);
        } else if p.extension().is_some_and(|x| x == "rs")
            && let Ok(src) = std::fs::read_to_string(&p)
        {
            let rel = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .into_owned();
            files.push((rel, src));
        }
    }
}

/// Every finding for `files`, and how many types state their messages.
fn findings(files: &[(String, String)]) -> (usize, Vec<String>) {
    let mut out = Vec::new();
    let mut stating = 0usize;
    for (rel, src) in files {
        if !rel.contains("/src/") {
            continue;
        }
        for (ty, names_message, has) in impls(src) {
            if has {
                stating += 1;
            } else if names_message {
                out.push(format!(
                    "{rel}  {ty}: an event_type names a message, and `verlauf` states none — \
                     give it `fn verlauf` (direction, PID, message reference)"
                ));
            }
        }
    }
    (stating, out)
}

pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    collect(&workspace_root.join("crates"), workspace_root, &mut files);
    if files.is_empty() {
        eprintln!("check-event-verlauf: no source file under crates/ — the layout has changed");
        return false;
    }
    let (stating, out) = findings(&files);
    if out.is_empty() {
        println!("check-event-verlauf: {stating} event type(s) state their messages");
        true
    } else {
        eprintln!("check-event-verlauf: {} finding(s):", out.len());
        for f in &out {
            eprintln!("  {f}");
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::findings;

    fn file(src: &str) -> Vec<(String, String)> {
        vec![("crates/x/src/lib.rs".to_owned(), src.to_owned())]
    }

    const SILENT: &str = r#"impl EventPayload for E {
        fn event_type(&self) -> &'static str { match self { Self::A => "AntwortGesendet" } }
    }"#;

    const STATING: &str = r#"impl EventPayload for E {
        fn event_type(&self) -> &'static str { match self { Self::A => "AntwortGesendet" } }
        fn verlauf(&self) -> Option<Verlauf> { None }
    }"#;

    #[test]
    fn a_message_event_that_states_nothing_is_refused() {
        let (_, out) = findings(&file(SILENT));
        assert_eq!(out.len(), 1, "{out:?}");
    }

    #[test]
    fn a_message_event_that_states_its_messages_passes() {
        let (stating, out) = findings(&file(STATING));
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(stating, 1);
    }

    #[test]
    fn a_german_inbound_name_is_read_as_a_message() {
        let src = r#"impl EventPayload for E {
            fn event_type(&self) -> &'static str { match self { Self::A => "AnweisungErhalten" } }
        }"#;
        assert_eq!(findings(&file(src)).1.len(), 1);
    }

    #[test]
    fn a_state_change_event_needs_nothing() {
        let src = r#"impl EventPayload for E {
            fn event_type(&self) -> &'static str { match self { Self::A => "Activated" } }
        }"#;
        assert!(findings(&file(src)).1.is_empty());
    }
}
