//! Every outbox `message_type` a workflow emits must reach a worker.
//!
//! An outbox entry is dispatched by exactly one of two workers, selected by its
//! `message_type`:
//!
//! - the **EDIFACT renderer** (`render_to_wire_bytes`), for wire messages, and
//! - the **ERP adapter** (`map_message_type_to_erp_event`), for domain intents
//!   delivered to the ERP webhook as CloudEvents.
//!
//! A type in neither is accepted by the outbox and then goes nowhere. The
//! failure is quiet by construction: the renderer returns `InsufficientPayload`
//! and the AS4 sender puts the raw domain **JSON** on the wire where an EDIFACT
//! interchange belonged — a log warning, a message the market partner cannot
//! parse, and no error anywhere.
//!
//! Source scanning is deliberate. The emitted set lives in constructor calls
//! across the domain crates and is not enumerable at runtime, so the test reads
//! the same thing a reviewer would.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // `target/` holds build artefacts, not authored source.
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every literal message type passed to `PendingOutbox::new`.
fn emitted_types() -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_sources(&workspace_root().join("crates"), &mut files);

    let mut out = BTreeSet::new();
    for f in files {
        let Ok(src) = std::fs::read_to_string(&f) else {
            continue;
        };
        let mut rest = src.as_str();
        while let Some(at) = rest.find("PendingOutbox::new(") {
            rest = &rest[at + "PendingOutbox::new(".len()..];
            // The first argument is the message type; it may sit on the next
            // line, so skip whitespace before requiring the opening quote.
            let arg = rest.trim_start();
            if let Some(lit) = arg.strip_prefix('"')
                && let Some(end) = lit.find('"')
            {
                out.insert(lit[..end].to_owned());
            }
        }
    }
    out
}

/// The message types a `match` in `path` dispatches on, from its arm literals.
fn handled_in(path: &Path, after_marker: &str) -> BTreeSet<String> {
    let src = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let start = src
        .find(after_marker)
        .unwrap_or_else(|| panic!("marker {after_marker:?} not found in {}", path.display()));
    let body = &src[start..];

    let mut out = BTreeSet::new();
    for line in body.lines() {
        let line = line.trim();
        if !line.contains("=>") {
            continue;
        }
        let Some(arms) = line.split("=>").next() else {
            continue;
        };
        // `"UTILMD" => …` and `"ProcessCompleted" | "ProcessComplete" => …`
        for piece in arms.split('|') {
            let piece = piece.trim();
            if let Some(lit) = piece.strip_prefix('"')
                && let Some(end) = lit.find('"')
                && lit[end + 1..].trim().is_empty()
            {
                out.insert(lit[..end].to_owned());
            }
        }
        // Stop at the end of the match — the catch-all arm.
        if arms.trim() == "other" || arms.trim() == "_" {
            break;
        }
    }
    out
}

#[test]
fn every_emitted_outbox_type_has_a_worker() {
    let root = workspace_root();
    let rendered = handled_in(
        &root.join("services/makod/src/orchestrator/edifact_renderer/mod.rs"),
        "match msg.message_type.as_ref() {",
    );
    let erp = handled_in(
        &root.join("services/makod/src/core/erp_adapter.rs"),
        "Some(match msg_type {",
    );

    assert!(
        rendered.len() >= 10,
        "only {} renderer arms parsed — the scan is broken, not the code: {rendered:?}",
        rendered.len()
    );
    assert!(
        erp.len() >= 6,
        "only {} ERP arms parsed — the scan is broken, not the code: {erp:?}",
        erp.len()
    );

    let emitted = emitted_types();
    assert!(
        emitted.len() >= 8,
        "only {} emitted types found — the scan is broken: {emitted:?}",
        emitted.len()
    );

    let orphans: Vec<&String> = emitted
        .iter()
        .filter(|t| !rendered.contains(*t) && !erp.contains(*t))
        .collect();

    assert!(
        orphans.is_empty(),
        "these outbox message types are emitted by a workflow but handled by \
         neither the EDIFACT renderer nor the ERP adapter, so the entry is \
         enqueued and then goes nowhere: {orphans:?}\n\n\
         Either rename the emitter to an existing wire type (e.g. `UTILMD`) and \
         match the renderer's payload contract, or add the renderer / ERP arm.\n\
         renderer handles: {rendered:?}\n\
         ERP adapter handles: {erp:?}"
    );
}

/// Whether `line` is a comment, which names a type without writing it.
fn is_comment(line: &str) -> bool {
    line.trim_start().starts_with("//")
}

/// The production text of `src`: comment lines dropped, and everything from
/// the first `#[cfg(test)]` on cut, so a test fixture is not a writer.
fn production_text(src: &str) -> String {
    let end = src.find("#[cfg(test)]").unwrap_or(src.len());
    src[..end]
        .lines()
        .filter(|l| !is_comment(l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every mapped ERP `message_type` has a writer.
///
/// The ERP adapter's map is read by a worker that only ever sees what some
/// producer enqueued. A type it maps that nothing writes is a CloudEvent with
/// a subscriber and no emitter: the consumer waits for a state — a rejected
/// APERAK, a failed process — that can never be projected, and nothing says
/// so. A writer is the type's literal in production code of `crates/` or of
/// makod outside the map itself: a `PendingOutbox::new("…"`, or an
/// `OutboxMessage` built by makod or the engine.
#[test]
fn every_mapped_message_type_has_a_writer() {
    let root = workspace_root();
    let map = root.join("services/makod/src/core/erp_adapter.rs");
    let mapped = handled_in(&map, "Some(match msg_type {");
    assert!(
        mapped.len() >= 6,
        "only {} ERP arms parsed — the scan is broken: {mapped:?}",
        mapped.len()
    );

    let mut files = Vec::new();
    rust_sources(&root.join("crates"), &mut files);
    rust_sources(&root.join("services/makod/src"), &mut files);
    let text: String = files
        .iter()
        .filter(|f| **f != map && !f.components().any(|c| c.as_os_str() == "tests"))
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .map(|src| production_text(&src))
        .collect::<Vec<_>>()
        .join("\n");

    let unwritten: Vec<&String> = mapped
        .iter()
        .filter(|t| !text.contains(&format!("\"{t}\"")))
        .collect();
    assert!(
        unwritten.is_empty(),
        "these message types are mapped to an ERP CloudEvent but no production \
         code writes them, so their subscribers never see one: {unwritten:?}"
    );
}
