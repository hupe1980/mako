//! Guard: the AS4 boundary's controls are reachable.
//!
//! The AS4 port is the only door a counterparty knocks on directly, and every
//! control on it fails in the same quiet way: the code exists, reads as though
//! it runs, and does not. Two shipped that way.
//!
//! **A declared floor nothing calls.** `BdewAs4Profile::validate()` asserts the
//! BDEW sign-and-encrypt mandate over the base policy layer *and* every override
//! — the implementation and its tests are correct — and its own doc says to call
//! it at startup before serving traffic. Nothing did. `asx-rs` refuses a policy
//! layer only when it disables signing **and** encryption, so a sign-only
//! override validates cleanly there and would have sent in the clear.
//!
//! **A session with no pinned signer.** The inbound pipeline must be handed a
//! session pinned to the signing certificate of the party a message claims to
//! come from. Handed the *base* session instead, two things go wrong at once:
//! nothing binds the claimed `eb:From` to the verified signer, and — because
//! `asx-rs` refuses a signed message whose session pins no fingerprint, while
//! BDEW AS4-Profil v1.2 §2.2.6.2.1 makes signing mandatory — the listener
//! accepts *nothing*, while its logs read as the counterparty's fault.
//!
//! Neither was visible to a test. The security suite asserted that a tampered
//! message is rejected, which a pipeline rejecting everything also satisfies,
//! and the floor's tests exercised the function rather than its call site.
//! A call site is what this guard checks, because that is the part that was
//! missing both times.
//!
//! ## What it refuses
//!
//! 1. `core/preflight.rs` that does not call `validate()` on the assembled
//!    profile.
//! 2. `transport/as4_ingest.rs` that hands the **base** session
//!    (`&self.session`) to the receive pipeline instead of a per-sender one.
//! 3. A refusal inside `handle_first_seen` that returns without first writing a
//!    dead letter.
//!
//! The third is what lets the caller settle the dedup claim with a single
//! unconditional `accept`. The dedup key is spent before the message is
//! dispatched, so a `ReceptionAwareness` retransmission — same `eb:MessageId` —
//! is answered as a duplicate. A refusal path that returned without recording
//! the message would therefore lose it *and* tell the counterparty it arrived.
//! Every such path currently dead-letters; this keeps that true, because the
//! settlement above it is only correct while it is.
//!
//! ## Why the files are named
//!
//! A pattern search over the tree would pass on a tree where the file moved,
//! which is the failure mode every floor in these guards exists to refuse. The
//! paths are stated, and a missing one is a failure that has to be taught, not
//! skipped.

use std::path::Path;

/// One required call site: a file, the text that must appear, and why.
struct Required {
    /// Workspace-relative path.
    file: &'static str,
    /// Text that must appear in it.
    needle: &'static str,
    /// What its absence means.
    reason: &'static str,
}

/// Call sites whose absence is a live control gap.
const REQUIRED: &[Required] = &[Required {
    file: "services/makod/src/core/preflight.rs",
    needle: "as4_profile.validate()",
    reason: "the BDEW sign-and-encrypt floor is asserted by `BdewAs4Profile::validate()`, and \
             it is only a control when the path that assembles the deployed profile runs it. \
             `asx-rs` refuses a layer only when it disables signing AND encryption, so a \
             sign-only override passes there and sends in the clear",
}];

/// Text that must **not** appear, because it names a control being bypassed.
struct Refused {
    /// Workspace-relative path.
    file: &'static str,
    /// Text that may not appear.
    needle: &'static str,
    /// What its presence means.
    reason: &'static str,
}

/// Call shapes that defeat a control at the AS4 boundary.
const REFUSED: &[Refused] = &[Refused {
    file: "services/makod/src/transport/as4_ingest.rs",
    needle: "receive_push_with_dedup_async(\n            &self.session,",
    reason: "the inbound pipeline must be handed the session pinned to the signing certificate \
             of the sender the message claims to be, not the base session. The base session \
             pins no fingerprint, and `asx-rs` refuses every signed message when none is \
             pinned — so this both unbinds the sender and takes the listener offline",
}];

/// Scan the named AS4 control sites.
///
/// Returns `true` when every required call site is present and no bypass shape
/// appears.
#[must_use]
pub fn run(workspace_root: &Path) -> bool {
    let mut findings: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for req in REQUIRED {
        let path = workspace_root.join(req.file);
        let Ok(src) = std::fs::read_to_string(&path) else {
            findings.push(format!(
                "{}: not readable — this guard names the file deliberately, so a move has to \
                 be taught to it rather than silently skipping the check",
                req.file
            ));
            continue;
        };
        checked += 1;
        if !src.contains(req.needle) {
            findings.push(format!(
                "{}: `{}` is not called.\n      {}",
                req.file, req.needle, req.reason
            ));
        }
    }

    for refused in REFUSED {
        let path = workspace_root.join(refused.file);
        let Ok(src) = std::fs::read_to_string(&path) else {
            findings.push(format!(
                "{}: not readable — this guard names the file deliberately, so a move has to \
                 be taught to it rather than silently skipping the check",
                refused.file
            ));
            continue;
        };
        checked += 1;
        // Compare with whitespace collapsed, so reformatting does not defeat it.
        if collapse(&src).contains(&collapse(refused.needle)) {
            findings.push(format!(
                "{}: the base session reaches the receive pipeline.\n      {}",
                refused.file, refused.reason
            ));
        }
    }

    // Rule 3 — the refusal paths that the dedup settlement depends on.
    let ingest = workspace_root.join("services/makod/src/transport/as4_ingest.rs");
    match std::fs::read_to_string(&ingest) {
        Ok(src) => {
            checked += 1;
            let offenders = undead_lettered_refusals(&src);
            if offenders == vec![usize::MAX] {
                findings.push(
                    "services/makod/src/transport/as4_ingest.rs: `handle_first_seen` is gone — \
                     the dedup claim is settled on the assumption that every refusal inside it \
                     records the message first, so this guard has to be taught the new shape"
                        .to_owned(),
                );
            } else {
                for line in offenders {
                    findings.push(format!(
                        "services/makod/src/transport/as4_ingest.rs:{line}: refuses without \
                         writing a dead letter first.\n      The dedup key is already spent, so \
                         the retransmission is answered as a duplicate — the message is lost and \
                         the counterparty holds an acknowledgement for it."
                    ));
                }
            }
        }
        Err(_) => {
            findings.push("services/makod/src/transport/as4_ingest.rs: not readable".to_owned())
        }
    }

    if checked < REQUIRED.len() + REFUSED.len() + 1 {
        println!(
            "check-as4-controls: FAIL — {checked} of {} named site(s) were readable",
            REQUIRED.len() + REFUSED.len() + 1
        );
        for f in &findings {
            println!("  {f}");
        }
        return false;
    }

    if findings.is_empty() {
        println!(
            "check-as4-controls: every AS4 control at the {checked} named site(s) is reachable"
        );
        return true;
    }

    println!(
        "check-as4-controls: FAIL — {} AS4 control(s) are not reachable:\n",
        findings.len()
    );
    for f in &findings {
        println!("  {f}");
    }
    println!(
        "\nThe AS4 port is the only door a counterparty knocks on directly, and a control there \
         fails silently: the code reads as though it runs. Restore the call site."
    );
    false
}

/// Refusals inside `handle_first_seen` that write no dead letter first.
///
/// Textual and deliberately local: a `dl_sink` reject within the preceding
/// [`DEAD_LETTER_WINDOW`] lines counts. That is how these refusals are written
/// — record, log, return — and a check that tried to prove reachability would
/// need the control-flow graph to say anything at all.
///
/// **What it therefore does not catch:** a new refusal added *just after*
/// another path's dead letter sits inside that path's window and passes. The
/// check finds a refusal written somewhere fresh, which is how one actually
/// arrives; it does not prove that the dead letter above any given refusal is
/// on the same branch. Worth knowing before trusting it as the only argument —
/// the settlement it protects is in `as4_ingest.rs`, and the reasoning belongs
/// next to it as well as here.
fn undead_lettered_refusals(src: &str) -> Vec<usize> {
    let lines: Vec<&str> = src.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|l| l.contains("async fn handle_first_seen"))
    else {
        return vec![usize::MAX];
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(start) {
        if !line.contains("HandlerOutcome::bad_request") {
            continue;
        }
        let from = i.saturating_sub(DEAD_LETTER_WINDOW).max(start);
        let window = lines[from..i].join("\n");
        if !(window.contains("dl_sink") && window.contains("reject(")) {
            out.push(i + 1);
        }
    }
    out
}

/// How far back a dead letter may sit from the refusal it belongs to.
const DEAD_LETTER_WINDOW: usize = 30;

/// Strip every whitespace character, so formatting is not load-bearing.
///
/// Removed rather than collapsed to a single space: `rustfmt` decides whether a
/// call is `f(&self.session, …)` or wrapped onto its own line, and collapsing
/// leaves those two as `f( &self.session,` and `f(&self.session,` — different
/// strings, so the guard would pass on exactly the formatting it did not
/// anticipate.
fn collapse(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The needle matches whichever way `rustfmt` wrapped the call.
    #[test]
    fn whitespace_is_not_load_bearing() {
        assert_eq!(collapse("a\n   b\t c"), "abc");
        let needle = collapse("receive_push_with_dedup_async(\n            &self.session,");
        for shape in [
            "receive_push_with_dedup_async(&self.session, &bus, req, dedup)",
            "receive_push_with_dedup_async(\n    &self.session,\n    &bus,\n)",
            "receive_push_with_dedup_async(\n            &self.session,\n",
        ] {
            assert!(
                collapse(shape).contains(&needle),
                "the needle must survive this wrapping: {shape}"
            );
        }
    }

    /// A tree missing the required call site is refused.
    ///
    /// A guard that cannot fail certifies nothing, so this is the input that
    /// makes it fail.
    #[test]
    fn a_missing_call_site_is_refused() {
        let dir = std::env::temp_dir().join("mako-check-as4-controls-missing");
        let _ = std::fs::remove_dir_all(&dir);
        for r in REQUIRED {
            let p = dir.join(r.file);
            std::fs::create_dir_all(p.parent().expect("parent")).unwrap();
            std::fs::write(&p, "fn preflight() {}\n").unwrap();
        }
        for r in REFUSED {
            let p = dir.join(r.file);
            std::fs::create_dir_all(p.parent().expect("parent")).unwrap();
            std::fs::write(&p, "fn handle() {}\n").unwrap();
        }
        assert!(!run(&dir), "a missing validate() call site must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tree handing the base session to the receive pipeline is refused,
    /// including when rustfmt has rewrapped the call.
    #[test]
    fn the_base_session_reaching_the_pipeline_is_refused() {
        let dir = std::env::temp_dir().join("mako-check-as4-controls-bypass");
        let _ = std::fs::remove_dir_all(&dir);
        for r in REQUIRED {
            let p = dir.join(r.file);
            std::fs::create_dir_all(p.parent().expect("parent")).unwrap();
            std::fs::write(&p, "let _ = as4_profile.validate()?;\n").unwrap();
        }
        for r in REFUSED {
            let p = dir.join(r.file);
            std::fs::create_dir_all(p.parent().expect("parent")).unwrap();
            std::fs::write(
                &p,
                "match receive_push_with_dedup_async(&self.session, &bus, req, dedup).await {}\n\
                 async fn handle_first_seen(&self) -> HandlerOutcome {\n\
                 self.ingest.dl_sink.reject(&reason);\n\
                 HandlerOutcome::bad_request(\"refused\")\n}\n",
            )
            .unwrap();
        }
        assert!(!run(&dir), "the base session must not reach the pipeline");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A tree with both controls in place passes.
    #[test]
    fn the_controls_in_place_pass() {
        let dir = std::env::temp_dir().join("mako-check-as4-controls-ok");
        let _ = std::fs::remove_dir_all(&dir);
        for r in REQUIRED {
            let p = dir.join(r.file);
            std::fs::create_dir_all(p.parent().expect("parent")).unwrap();
            std::fs::write(&p, "let _report = as4_profile.validate().map_err(x)?;\n").unwrap();
        }
        for r in REFUSED {
            let p = dir.join(r.file);
            std::fs::create_dir_all(p.parent().expect("parent")).unwrap();
            std::fs::write(
                &p,
                "receive_push(&self.session, &bus, push).await\n\
                 async fn handle_first_seen(&self) -> HandlerOutcome {\n\
                 self.ingest.dl_sink.reject(&reason);\n\
                 HandlerOutcome::bad_request(\"refused\")\n}\n",
            )
            .unwrap();
        }
        assert!(run(&dir), "a tree with both controls in place must pass");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A refusal that records nothing is refused.
    ///
    /// This is the shape the dedup settlement cannot survive: the key is spent,
    /// so the retransmission is a duplicate and the message is gone.
    #[test]
    fn a_refusal_without_a_dead_letter_is_refused() {
        let src = "async fn handle_first_seen(&self) -> HandlerOutcome {\n                   return HandlerOutcome::bad_request(\"nothing recorded\");\n}\n";
        assert_eq!(undead_lettered_refusals(src).len(), 1);
    }

    /// A refusal that dead-letters first passes.
    #[test]
    fn a_refusal_that_records_first_passes() {
        let src = "async fn handle_first_seen(&self) -> HandlerOutcome {\n                   self.ingest.dl_sink.reject(&reason);\n                   HandlerOutcome::bad_request(\"recorded\")\n}\n";
        assert!(undead_lettered_refusals(src).is_empty());
    }

    /// A named file that is not readable is a failure, not a skip.
    #[test]
    fn a_moved_file_is_refused() {
        let dir = std::env::temp_dir().join("mako-check-as4-controls-moved");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!run(&dir), "a missing named site must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
