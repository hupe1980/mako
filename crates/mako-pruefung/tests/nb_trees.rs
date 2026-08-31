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

// ── Reachability: every code the tree publishes must have a way in ────────────

/// `E_0607` publishes fourteen codes across two branches. This walks the input
/// space and asserts which of them a decision can actually reach.
///
/// The mirror of `validate-ebd-codes`: that check holds the catalogue against
/// the document, this one holds the *walk* against the catalogue. It guards two
/// opposite failures — a code that is catalogued but that no Prüfschritt emits,
/// and a code a refactor quietly moved the branch out from under.
///
/// A code the projection cannot decide belongs in `UNREACHABLE` with its
/// reason. Leaving one out of both lists fails.
#[test]
fn e_0607_reaches_every_code_it_can_decide() {
    use mako_markt::{
        domain::Sparte,
        repository::{LfZuordnung, LieferStatus, VersorgungsStatusRecord, ZuordnungsStatus},
    };
    use mako_pruefung::nb::types::{AbmeldungAnfrage, Marktlokationsart, Messtyp};
    use std::collections::BTreeSet;
    use time::macros::{date, datetime};

    /// Codes `E_0607` publishes that no input can reach, and why.
    const UNREACHABLE: &[(&str, &str)] = &[
        (
            "A01",
            "Prüfschritte 20–30 ask whether the Marktlokation is a „ruhende \
             Marktlokation\" of a Kundenanlage (§ 20 Abs. 1d EnWG / § 10c EEG); nothing \
             records that",
        ),
        (
            "A10",
            "Prüfschritt 130 turns on the *already confirmed* Abmeldung's \
             Transaktionsgrund — an earlier message the projection does not keep — so the \
             decision escalates rather than choose between A10 and confirming",
        ),
        ("A26", "the erzeugend twin of A10, at Prüfschritt 610"),
        (
            "A99",
            "„ist ein zuvor nicht spezifizierter Fehler aufgetreten?\" is an operator's \
             finding, not a decision this engine makes; its Nutzungsmöglichkeit ends \
             01.04.2027 anyway",
        ),
    ];

    // Monday 2026-03-02 09:00 UTC — the same NOW the unit tests use.
    let now = datetime!(2026-03-02 09:00 UTC);
    let config = mako_pruefung::NetzCheckConfig::default();

    let anfrage = |art, grund: Option<&str>, abmeldedatum| AbmeldungAnfrage {
        pid: 55_004,
        process_id: uuid::Uuid::new_v4(),
        malo_id: "51238696012".to_owned(),
        lf_mp_id: "9900357000004".to_owned(),
        grid_operator_gln: "9900000000002".to_owned(),
        abmeldedatum,
        sparte: Sparte::Strom,
        messtyp: Messtyp::Slp,
        transaktionsgrund: grund.map(ToOwned::to_owned),
        marktlokationsart: art,
        erzeugung: None,
    };
    let versorgung =
        |lieferstatus, zugeordnet: bool, lieferende, beginn, eog_seit| VersorgungsStatusRecord {
            malo_id: "51238696012".parse().expect("valid MaLo"),
            lieferstatus,
            zuordnungen: if zugeordnet {
                vec![LfZuordnung {
                    zuordnungsbeginn: beginn,
                    ..LfZuordnung::ganz("9900357000004", ZuordnungsStatus::Aktiv)
                }]
            } else {
                Vec::new()
            },
            lieferende,
            msb_mp_id: None,
            nb_mp_id: "9900000000002".to_owned(),
            eog_seit,
            last_process_id: None,
            updated_at: time::OffsetDateTime::now_utc(),
            tenant: "9900000000002".to_owned(),
            version: 1,
        };

    // The dates are chosen to land on each Vorlauffrist branch, because the two
    // branches measure differently and a single „too close" date only refuses
    // one of them:
    //   05-01  a Monatserster far enough ahead — both branches pass
    //   05-15  not a Monatserster — erzeugend refuses A21, verbrauchend passes
    //   03-03  one day after receipt, no full Werktag between — verbrauchend A02
    //   04-01  a Monatserster whose latest ÜT (03-01) has passed — erzeugend A22
    let daten = [
        date!(2026 - 05 - 01),
        date!(2026 - 05 - 15),
        date!(2026 - 03 - 03),
        date!(2026 - 04 - 01),
    ];
    let gruende = [
        None,
        Some("E01"),
        Some("E02"),
        Some("Z33"),
        Some("ZH2"),
        Some("Z41"),
        Some("ZT4"),
    ];
    let arten = [
        Marktlokationsart::Verbrauchend,
        Marktlokationsart::Ruhend,
        Marktlokationsart::Erzeugend,
    ];

    let mut reached: BTreeSet<&str> = BTreeSet::new();
    for art in arten {
        for grund in gruende {
            for d in daten {
                // Every supply shape the projection can actually hold, plus the
                // two the Aufhebung and ESV steps read.
                let lagen = [
                    versorgung(LieferStatus::Beliefert, true, None, Some(d), None),
                    versorgung(
                        LieferStatus::Beliefert,
                        true,
                        None,
                        Some(date!(2026 - 06 - 01)),
                        None,
                    ),
                    versorgung(LieferStatus::Unbeliefert, false, Some(d), None, None),
                    versorgung(
                        LieferStatus::Ersatzversorgung,
                        true,
                        None,
                        Some(d),
                        Some(date!(2026 - 03 - 01)),
                    ),
                    versorgung(
                        LieferStatus::Ersatzversorgung,
                        true,
                        None,
                        Some(d),
                        Some(date!(2025 - 01 - 01)),
                    ),
                ];
                for vs in &lagen {
                    let out = mako_pruefung::evaluate_abmeldung(
                        &anfrage(art, grund, d),
                        Some(vs),
                        now,
                        &config,
                    );
                    if let Some(code) = out.antwortcode() {
                        reached.insert(
                            mako_pruefung::codes::lookup(
                                mako_pruefung::codes::EBD_ABMELDUNG_NB,
                                code,
                            )
                            .map(|c| c.code)
                            .unwrap_or_else(|| panic!("{code} is not published by E_0607")),
                        );
                    }
                }
            }
        }
    }

    let published: BTreeSet<&str> = mako_pruefung::codes::E_0607_CODES
        .iter()
        .map(|c| c.code)
        .collect();
    let excused: BTreeSet<&str> = UNREACHABLE.iter().map(|(c, _)| *c).collect();

    let unexplained: Vec<&str> = published
        .difference(&reached)
        .filter(|c| !excused.contains(*c))
        .copied()
        .collect();
    assert!(
        unexplained.is_empty(),
        "E_0607 publishes {unexplained:?} and no input reaches them. Either the Prüfschritt \
         is not implemented — in which case say so in UNREACHABLE — or a refactor moved the \
         branch out from under it."
    );

    let stale: Vec<&str> = excused.intersection(&reached).copied().collect();
    assert!(
        stale.is_empty(),
        "{stale:?} are listed as unreachable in E_0607 but an input reaches them — the list \
         has outlived the limitation it records"
    );
}
