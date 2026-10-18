//! Guard: the AS4 boundary's controls are reachable.
//!
//! The AS4 port is the only door a counterparty knocks on directly, and every
//! control on it fails in the same quiet way: the code exists, reads as though
//! it runs, and does not. `asx-rs` 0.16 owns the cryptography — `Profile::bdew()`
//! cannot be relaxed — so what remains on mako's side is the call sites that
//! make the library's checks apply to the deployment that boots, and the
//! settlement order the inbox depends on.
//!
//! ## What it requires
//!
//! 1. `core/preflight.rs` builds the AS4 clients (`As4Clients::build`): that
//!    runs `As4::builder(..).build()` per local MP-ID, which is where a signer
//!    outside the SM-PKI root, a certificate without the MP-ID in its OU or a
//!    missing decrypter is refused — at boot and under `--check`, not on the
//!    first delivery.
//! 2. `transport/as4_clients.rs` calls `build()` on the configured builder.
//! 3. `crates/mako-as4/src/identity.rs` registers the CRL signer and compares
//!    the three key pairs by public key — the two checks `build()` does not do
//!    for mako (it does not require a CRL signer, and it compares certificates).
//! 4. `transport/as4_ingest.rs` consults the sender policy
//!    (`[policy.as4] unbekannte_absender`) before dispatch.
//!
//! ## What it refuses
//!
//! 5. An in-memory dedup store (`MemoryDedup`) in the inbox wiring: it forgets
//!    every accepted key on restart, so a retransmission inside the sender's
//!    retry window is processed twice.
//! 6. A permanent refusal (`Reject::permanent`) in the inbox handler that is
//!    not preceded by an awaited dead-letter write. A permanent refusal
//!    answers `EBMS:0004`, which the sender does not retry; without the dead
//!    letter the message is gone and nobody holds a record of it.
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
const REQUIRED: &[Required] = &[
    Required {
        file: "services/makod/src/core/preflight.rs",
        needle: "As4Clients::build(",
        reason: "the AS4 clients are built — `As4::builder(..).build()` per local MP-ID — on the \
                 path `--check` and the boot share; without it a key-material refusal surfaces \
                 on the first delivery instead of at startup",
    },
    Required {
        file: "services/makod/src/transport/as4_clients.rs",
        needle: "configure(builder).build()",
        reason: "`build()` is where asx-rs validates the identity against the BDEW profile; a \
                 client assembled without it is never checked",
    },
    Required {
        file: "crates/mako-as4/src/identity.rs",
        needle: ".crl_signer(crl_signer)",
        reason: "the SM-PKI CCRL-S(Root) signs the Root-CA CRL; without it registered no \
                 certificate below the root passes the revocation check",
    },
    Required {
        file: "crates/mako-as4/src/identity.rs",
        needle: "same_public_key(a, b)",
        reason: "BDEW AS4-Profil 1.2 §2.3.4.4 requires different key pairs for signing, \
                 encryption and TLS; asx-rs compares certificates, so two certificates of one \
                 key pair pass it",
    },
    Required {
        file: "services/makod/src/transport/as4_ingest.rs",
        needle: "self.clients.admits_sender(",
        reason: "`[policy.as4] unbekannte_absender = nur_konfigurierte` is enforced by this \
                 call alone; without it every SM-PKI certificate holder is served",
    },
];

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
const REFUSED: &[Refused] = &[
    Refused {
        file: "services/makod/src/transport/as4_ingest.rs",
        needle: "MemoryDedup",
        reason: "the inbox dedup store must be durable: an in-memory one forgets accepted keys \
                 on restart, and a retransmission is processed twice",
    },
    Refused {
        file: "services/makod/src/startup/servers.rs",
        needle: "MemoryDedup",
        reason: "the inbox dedup store must be durable: an in-memory one forgets accepted keys \
                 on restart, and a retransmission is processed twice",
    },
];

/// Scan the named AS4 control sites.
///
/// Returns `true` when every required call site is present and no bypass shape
/// appears.
#[must_use]
pub fn run(workspace_root: &Path) -> bool {
    let mut findings: Vec<String> = Vec::new();
    let mut checked = 0usize;
    let read = |file: &str, findings: &mut Vec<String>| {
        let src = std::fs::read_to_string(workspace_root.join(file));
        if src.is_err() {
            findings.push(format!(
                "{file}: not readable — this guard names the file deliberately, so a move has to \
                 be taught to it rather than silently skipping the check"
            ));
        }
        src.ok()
    };

    for req in REQUIRED {
        let Some(src) = read(req.file, &mut findings) else {
            continue;
        };
        checked += 1;
        if !collapse(&src).contains(&collapse(req.needle)) {
            findings.push(format!(
                "{}: `{}` is not called.\n      {}",
                req.file, req.needle, req.reason
            ));
        }
    }

    for refused in REFUSED {
        let Some(src) = read(refused.file, &mut findings) else {
            continue;
        };
        checked += 1;
        if collapse(&src).contains(&collapse(refused.needle)) {
            findings.push(format!(
                "{}: `{}` appears.\n      {}",
                refused.file, refused.needle, refused.reason
            ));
        }
    }

    // Rule 6 — the permanent refusals of the inbox handler.
    let ingest = "services/makod/src/transport/as4_ingest.rs";
    if let Some(src) = read(ingest, &mut findings) {
        checked += 1;
        let offenders = undead_lettered_refusals(&src);
        if offenders == vec![usize::MAX] {
            findings.push(format!(
                "{ingest}: `BdewAs4IngestHandler::handle` is gone — the permanent refusals are \
                 checked from it, so this guard has to be taught the new shape"
            ));
        } else {
            for line in offenders {
                findings.push(format!(
                    "{ingest}:{line}: refuses permanently without writing a dead letter first.\n      \
                     `Reject::permanent` answers EBMS:0004, which the sender does not retry — \
                     the message is gone and nothing records it."
                ));
            }
        }
    }

    let sites = REQUIRED.len() + REFUSED.len() + 1;
    if checked < sites {
        println!("check-as4-controls: FAIL — {checked} of {sites} named site(s) were readable");
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

/// Permanent refusals in the inbox handler that write no dead letter first.
///
/// Scans from `pub async fn handle(` — the handler and `handle_first_seen`
/// below it. Textual and deliberately local: an awaited `dl_sink` reject within
/// the preceding [`DEAD_LETTER_WINDOW`] lines counts. That is how these
/// refusals are written — record, log, refuse — and a check that tried to prove
/// reachability would need the control-flow graph to say anything at all.
///
/// **What it therefore does not catch:** a new refusal added *just after*
/// another path's dead letter sits inside that path's window and passes. It
/// finds a refusal written somewhere fresh, which is how one actually arrives;
/// the settlement it protects is documented in `as4_ingest.rs` as well.
fn undead_lettered_refusals(src: &str) -> Vec<usize> {
    let lines: Vec<&str> = src.lines().collect();
    let Some(start) = lines
        .iter()
        .position(|l| l.contains("pub async fn handle("))
    else {
        return vec![usize::MAX];
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(start) {
        if !line.contains("Reject::permanent(") {
            continue;
        }
        let from = i.saturating_sub(DEAD_LETTER_WINDOW).max(start);
        let window = lines[from..i].join("\n");
        if !(window.contains("dl_sink") && awaited_reject(&window)) {
            out.push(i + 1);
        }
    }
    out
}

/// Whether the window's last `reject(` is awaited before the refusal.
///
/// The sink returns once the dead letter is durable; a `reject(…)` that is not
/// awaited is a future dropped on the floor. Whitespace is stripped first,
/// because rustfmt wraps the argument list over several lines.
fn awaited_reject(window: &str) -> bool {
    let flat: String = window.chars().filter(|c| !c.is_whitespace()).collect();
    flat.rfind("reject(")
        .is_some_and(|at| flat[at..].contains(".await"))
}

/// How far back a dead letter may sit from the refusal it belongs to.
const DEAD_LETTER_WINDOW: usize = 30;

/// Strip every whitespace character, so formatting is not load-bearing.
fn collapse(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HANDLER_OK: &str = "pub async fn handle(&self) -> Result<(), Reject> {\n\
        if !self.clients.admits_sender(&from) { return Err(Reject::new(\"x\")); }\n\
        self.ingest.dl_sink.reject(&reason).await?;\n\
        return Err(Reject::permanent(\"recorded\"));\n}\n";

    /// A tree with every site in place.
    fn tree(name: &str, ingest: &str, servers: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mako-check-as4-controls-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        let w = |file: &str, body: &str| {
            let p = dir.join(file);
            std::fs::create_dir_all(p.parent().expect("parent")).unwrap();
            std::fs::write(&p, body).unwrap();
        };
        w(
            "services/makod/src/core/preflight.rs",
            "Some(setup) => Some(As4Clients::build(setup)?),\n",
        );
        w(
            "services/makod/src/transport/as4_clients.rs",
            "let client = configure(builder).build().map_err(e)?;\n",
        );
        w(
            "crates/mako-as4/src/identity.rs",
            "if same_public_key(a, b) {}\n.crl_signer(crl_signer);\n",
        );
        w("services/makod/src/transport/as4_ingest.rs", ingest);
        w("services/makod/src/startup/servers.rs", servers);
        dir
    }

    #[test]
    fn the_controls_in_place_pass() {
        let dir = tree("ok", HANDLER_OK, "SlateDbDedup::new(store)\n");
        assert!(run(&dir), "a tree with every control in place must pass");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A guard that cannot fail certifies nothing: each required site removed
    /// is refused.
    #[test]
    fn a_missing_call_site_is_refused() {
        for req in REQUIRED {
            let dir = tree("missing", HANDLER_OK, "");
            let path = dir.join(req.file);
            let src = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, src.replace(req.needle, "removed()")).unwrap();
            assert!(!run(&dir), "removing `{}` must be refused", req.needle);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn an_in_memory_dedup_store_is_refused() {
        let dir = tree(
            "memory",
            HANDLER_OK,
            "client.inbound(asx::MemoryDedup::default(), h)\n",
        );
        assert!(!run(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shape the inbox cannot survive: a permanent refusal with the
    /// awaited dead letter removed.
    #[test]
    fn a_permanent_refusal_without_a_dead_letter_is_refused() {
        let src = HANDLER_OK.replace("self.ingest.dl_sink.reject(&reason).await?;\n", "");
        assert_eq!(undead_lettered_refusals(&src).len(), 1);
        let dir = tree("undead", &src, "");
        assert!(!run(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_permanent_refusal_that_records_first_passes() {
        assert!(undead_lettered_refusals(HANDLER_OK).is_empty());
    }

    /// A dead letter that is not awaited records nothing, so it is refused.
    #[test]
    fn an_unawaited_dead_letter_is_refused() {
        let src = HANDLER_OK.replace(".reject(&reason).await?;", ".reject(&reason);");
        assert_eq!(undead_lettered_refusals(&src).len(), 1);
    }

    /// A dead letter whose arguments rustfmt wrapped still counts once awaited.
    #[test]
    fn a_wrapped_awaited_dead_letter_passes() {
        let src = "pub async fn handle(&self) -> Result<(), Reject> {\n    self.ingest\n        .dl_sink\n        .reject(&DeadLetterReason::ProcessingError {\n            message: m(),\n        })\n        .await?;\n    return Err(Reject::permanent(\"recorded\"));\n}\n";
        assert!(undead_lettered_refusals(src).is_empty());
    }

    /// The real handler passes: every permanent refusal in today's
    /// `as4_ingest.rs` writes its dead letter first.
    #[test]
    fn todays_handler_passes() {
        let src = include_str!("../../services/makod/src/transport/as4_ingest.rs");
        assert!(undead_lettered_refusals(src).is_empty());
        assert!(
            src.matches("Reject::permanent(").count() >= 6,
            "the handler's permanent refusals are what this guard is about"
        );
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
