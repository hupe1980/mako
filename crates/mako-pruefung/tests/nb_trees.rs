//! The NB Entscheidungsbäume, and the wire obligations they create.
//!
//! Each test names the EBD and the Prüfschritt it pins, so a future edit that
//! moves a landing has to argue with the document rather than with a number.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 (01.04.2026)
//! and UTILMD AHB Strom 2.1 / 2.2.

// The NB trees only exist in a build that carries the NB role — an `lf-only` or
// `msb-only` binary compiles none of them, and nor should this.
#![cfg(feature = "role-nb")]

// ── The wire obligation the tree creates ─────────────────────────────────────

/// `E_0623` decides which Antwortcodes oblige the NB to restate the LFA's own
/// ground; `edi-energy` refuses to render an Ablehnung that omits it. The two
/// lists live in different crates because neither may depend on the other — a
/// domain crate that pulled in the wire library would make every role build
/// carry it — so this is where they are pinned.
///
/// A code added to one and not the other either puts an Ablehnung on the wire
/// that the receiving AHB rejects (`SG4 STS+Z35` missing where Bedingung
/// `[356]`/`[84]` marks it Muss), or refuses one the AHB accepts.
#[test]
fn the_tree_and_the_wire_agree_on_which_codes_oblige_a_sts_z35() {
    assert_eq!(
        mako_pruefung::CODES_REQUIRING_DRITTER,
        edi_energy::utilmd_codes::CODES_REQUIRING_DRITTER,
        "E_0623's LFA-Widerspruch codes and the render-time guard have drifted"
    );
}

/// Both codes are real `E_0623` Ablehnungen, and both are the Widerspruch
/// outcome — not, say, a Fristüberschreitung that happens to share a number.
#[test]
fn every_code_obliging_a_sts_z35_is_an_e0623_ablehnung() {
    for code in mako_pruefung::CODES_REQUIRING_DRITTER {
        let resolved = mako_pruefung::codes::lookup(mako_pruefung::codes::EBD_LIEFERBEGINN, code)
            .unwrap_or_else(|| panic!("E_0623 publishes {code}"));
        assert_eq!(resolved.cluster, mako_pruefung::codes::Cluster::Ablehnung);
        assert!(
            resolved.bedeutung.contains("widersprochen"),
            "{code} is not the LFA-Widerspruch outcome: {}",
            resolved.bedeutung
        );
    }
}
