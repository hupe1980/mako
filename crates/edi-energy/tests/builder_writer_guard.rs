//! Guards for the builder writer discipline.
//!
//! `emit_seg!` writes through `Writer::write_raw`, where a `:` inside an
//! element string is a component boundary. That is only safe for compile-time
//! constants: a runtime value carrying a literal separator would be silently
//! split instead of escaped. Runtime data must go through `emit_comp!`
//! (`Writer::write_composites`), where boundaries are structural and values
//! are escaped.
//!
//! The text guard pins the rule; the behavioral tests prove the property the
//! rule exists for — separator-hostile runtime values survive a round-trip.

use std::fmt::Write as _;
use std::path::Path;

/// Collect every `emit_seg!(...)` invocation (balanced-paren span) in a file.
fn emit_seg_calls(src: &str) -> Vec<String> {
    let mut calls = Vec::new();
    let mut rest = src;
    while let Some(idx) = rest.find("emit_seg!(") {
        let after = &rest[idx..];
        let mut depth = 0usize;
        let mut end = 0usize;
        for (i, c) in after.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        calls.push(after[..=end].to_owned());
        rest = &after[end..];
    }
    calls
}

/// No `emit_seg!` call may interpolate runtime data with `format!` — that is
/// what `emit_comp!` is for. A `format!` inside `emit_seg!` reintroduces the
/// class of bug where a `:` inside a runtime value becomes a component
/// boundary on the wire.
#[test]
fn no_format_inside_emit_seg() {
    let builders = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builders");
    let mut offenders = String::new();
    for entry in std::fs::read_dir(&builders).expect("read builders dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read builder source");
        for call in emit_seg_calls(&src) {
            if call.contains("format!") {
                let _ = writeln!(offenders, "{}: {call}", path.display());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "emit_seg! must not carry format!-interpolated runtime data — \
         use emit_comp! with explicit components instead:\n{offenders}"
    );
}

/// The macros live once in `builders/mod.rs` — a local redefinition would
/// silently shadow the shared one and could drop the escaping discipline.
#[test]
fn writer_macros_are_defined_once() {
    let builders = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builders");
    for entry in std::fs::read_dir(&builders).expect("read builders dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs")
            || path.file_name().and_then(|n| n.to_str()) == Some("mod.rs")
        {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read builder source");
        assert!(
            !src.contains("macro_rules! emit_seg") && !src.contains("macro_rules! emit_comp"),
            "{}: defines a local writer macro — the shared ones live in builders/mod.rs",
            path.display()
        );
    }
}

/// The property the discipline exists for: an APERAK error text containing
/// the component separator and the release character arrives intact, escaped
/// on the wire rather than split into components.
#[cfg(feature = "aperak")]
#[test]
fn separator_hostile_free_text_round_trips() {
    use edi_energy::Release;
    use edi_energy::builders::AperakBuilder;

    let hostile = "Fehler: DE 0062? fehlt + Wert ungültig";
    let msg = AperakBuilder::new(Release::new("2.4a"))
        .sender("9900987654321")
        .receiver("9900123456789")
        .error_text(hostile)
        .build()
        .expect("build APERAK");

    let ftx = msg
        .segments()
        .iter()
        .find(|s| s.tag == "FTX")
        .expect("FTX segment present");
    let text = ftx.component_str(3, 0).unwrap_or("");
    assert_eq!(
        text, hostile,
        "the free text must survive the writer/parser round-trip unchanged"
    );
}
