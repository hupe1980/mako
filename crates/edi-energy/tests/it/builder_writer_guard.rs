//! Guards for the builder writer output.
//!
//! Both writer macros escape a value: `emit_seg!` writes through
//! `Writer::write_simple`, where a `:` inside an element is data, and
//! `emit_comp!` through `Writer::write_composites`, where the component
//! boundaries are the structure itself. The tests prove the property that
//! matters on the wire — separator-hostile runtime values survive a
//! round-trip, and `UNT` counts the message — and pin the DE 2379 discipline.

use std::fmt::Write as _;
use std::path::Path;

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

/// `UNT` DE 0074 must equal the number of segments in the message, counting
/// `UNH` and `UNT` themselves and **excluding** the interchange envelope.
///
/// The writer derives the count from its own bookkeeping. A writer that records
/// only some emit paths, or that counts over its whole lifetime rather than the
/// current message, produces an interchange that fails its own `UNT` check at
/// the counterparty — the class of defect a syntactically valid message hides,
/// because every segment is well-formed and only the tally is wrong.
#[cfg(feature = "aperak")]
#[test]
fn unt_segment_count_matches_the_message() {
    use edi_energy::Release;
    use edi_energy::builders::AperakBuilder;

    let msg = AperakBuilder::new(Release::new("2.4a"))
        .sender("9900987654321")
        .receiver("9900123456789")
        .error_text("Prüfidentifikator ungültig")
        .build()
        .expect("build APERAK");

    let segments = msg.segments();
    let unh = segments
        .iter()
        .position(|s| s.tag == "UNH")
        .expect("UNH present");
    let unt_idx = segments
        .iter()
        .position(|s| s.tag == "UNT")
        .expect("UNT present");
    let expected = unt_idx - unh + 1;

    let declared: usize = segments[unt_idx]
        .element_str(0)
        .expect("UNT DE 0074 present")
        .parse()
        .expect("DE 0074 is numeric");

    let tags: Vec<&str> = segments.iter().map(|s| s.tag.as_ref()).collect();
    assert_eq!(
        declared, expected,
        "UNT declared {declared} segments, message has {expected} (tags: {tags:?})"
    );
}

// ── DE 2379 discipline ───────────────────────────────────────────────────────

/// The Dokumentendatum is `303` in every AHB, so no builder may emit it as `102`.
///
/// `DTM+137` carries DE 2379 = `303` (`CCYYMMDDHHMMZZZ`) with `[931]` fixing
/// the zone to `+00`, in every Anwendungsfall of all 31 AHB documents across
/// both format-version generations. `102` appears in none of them.
#[test]
fn the_dokumentendatum_is_never_emitted_as_102() {
    let builders = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builders");
    let mut offenders = String::new();
    for entry in std::fs::read_dir(&builders).expect("read builders dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read builder source");
        for line in src.lines() {
            let line = line.trim();
            if line.starts_with("//") {
                continue;
            }
            if line.contains("\"137\"") && line.contains("\"102\"") {
                let _ = writeln!(offenders, "{}: {line}", path.display());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "DTM+137 is DE 2379 `303` in every EDI@Energy AHB:\n{offenders}"
    );
}

/// A value emitted in format `303` must carry the `[931]` zone.
///
/// `CCYYMMDDHHMMZZZ` without the `ZZZ` is not a `303` value — mako's own
/// `parse_dtm_datetime` rejects it, and so does a conformant counterparty. Every
/// emit site therefore goes through `ccyymmddhhmm_utc`, which appends `+00` to a
/// zone-less `CCYYMMDDHHMM` stamp and refuses anything else — a bare `CCYYMMDD`
/// above all, which padding would turn into UTC midnight (see
/// `roundtrip::utilmd_transaction_tests::a_bare_date_in_a_303_dtm_is_refused`).
#[test]
fn every_303_value_is_zone_normalised() {
    let builders = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builders");
    let mut offenders = String::new();
    for entry in std::fs::read_dir(&builders).expect("read builders dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs")
            || path.file_name().and_then(|n| n.to_str()) == Some("mod.rs")
        {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read builder source");
        for line in src.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || !trimmed.contains("\"303\"") {
                continue;
            }
            if trimmed.contains("emit_comp!") && !trimmed.contains("ccyymmddhhmm_utc") {
                let _ = writeln!(offenders, "{}: {trimmed}", path.display());
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a DE 2379 `303` value needs its `[931]` zone — route it through \
         `ccyymmddhhmm_utc`:\n{offenders}"
    );
}
