//! The WiM Messstellenbetrieb trees, checked against the tables that name them.
//!
//! Two catalogues have to agree on every WiM answer: `mako-fristen`'s
//! [`antwort::WIM`] says *when* the answer is due and which EBD decides it, and
//! `mako-pruefung`'s [`codes`] says what the answer may contain. A
//! disagreement means one of them is stale, and the stale one is silent.

use mako_fristen::antwort;
use mako_pruefung::codes::{self, Cluster};

/// Every WiM tree named in the Frist table must exist here, with the same
/// answer PIDs.
#[test]
fn the_frist_table_and_the_codelisten_name_the_same_trees() {
    for pid in [55_039_u32, 55_042, 55_051, 55_168, 17_002, 17_001] {
        let o = antwort::antwort_obligation(pid)
            .unwrap_or_else(|| panic!("PID {pid} has no Antwortfrist"));
        let ebd = o
            .ebd
            .unwrap_or_else(|| panic!("PID {pid} names no Entscheidungsbaum"));
        assert!(
            codes::CODELISTEN.iter().any(|(id, _)| *id == ebd),
            "{ebd} (PID {pid}) is named by the Frist table but has no Codeliste"
        );
    }
}

/// The four MSB-Wechsel trees and the two ORDRSP ones publish alphabets that
/// share **no** code with the GPKE trees they are most often confused with.
#[test]
fn the_wim_alphabets_are_disjoint_from_the_gpke_ones() {
    let gpke: Vec<&str> = codes::E_0622_CODES
        .iter()
        .chain(codes::E_0607_CODES)
        .map(|c| c.code)
        .collect();
    for (id, list) in [
        (codes::EBD_KUENDIGUNG_MSB, codes::E_0200_CODES),
        (codes::EBD_ANMELDUNG_MSB, codes::E_0201_CODES),
        (codes::EBD_ABMELDUNG_MSB, codes::E_0202_CODES),
        (codes::EBD_VERPFLICHTUNGSANFRAGE, codes::E_0240_CODES),
    ] {
        for c in list {
            assert!(
                !gpke.contains(&c.code),
                "{id} publishes {} which also appears in a GPKE tree — check the source",
                c.code
            );
        }
    }
}

/// The codes `processd` used to send. Neither is published by any WiM tree, so
/// every automatic MSB-Wechsel rejection mako emitted was a code the
/// counterparty's Codeliste does not contain.
#[test]
fn a02_and_a05_are_not_wim_msb_wechsel_codes() {
    for ebd in [
        codes::EBD_KUENDIGUNG_MSB,
        codes::EBD_ANMELDUNG_MSB,
        codes::EBD_ABMELDUNG_MSB,
        codes::EBD_VERPFLICHTUNGSANFRAGE,
    ] {
        for code in ["A02", "A05", "A06", "A07"] {
            assert!(
                codes::lookup(ebd, code).is_none(),
                "{ebd} must not resolve the GPKE code {code}"
            );
        }
    }
    // …and the same spellings *do* exist in the Messlokationsänderung trees
    // with unrelated meanings, which is why a code is only ever resolved
    // against the tree that publishes it.
    assert_eq!(
        codes::lookup(codes::EBD_MESSLOKATIONSAENDERUNG_NB, "A02").map(|c| c.cluster),
        Some(Cluster::Zustimmung)
    );
}

/// 19005/19006 are shared by two trees with different alphabets, so a code can
/// never be resolved from the answer PID.
#[test]
fn the_two_messlokationsaenderung_trees_share_pids_but_not_codes() {
    assert!(codes::lookup(codes::EBD_MESSLOKATIONSAENDERUNG_LF, "A03").is_some());
    assert!(codes::lookup(codes::EBD_MESSLOKATIONSAENDERUNG_NB, "A03").is_none());
}

/// 19016 is named „Ablehnung Gerätewechselabsicht" but carries `ZB5` „Kein
/// Eigenausbau des MSBA" — a division of labour, not a refusal. Reading it as
/// one aborts a Gerätewechsel the counterparty just agreed to carry out.
#[test]
fn the_geraetewechselabsicht_answer_is_not_a_refusal() {
    let zb5 = codes::lookup(codes::EBD_GERAETEWECHSELABSICHT, "ZB5").expect("published");
    assert_eq!(zb5.cluster, Cluster::Ablehnung);
    assert!(zb5.bedeutung.contains("Eigenausbau"));
    let zb4 = codes::lookup(codes::EBD_GERAETEWECHSELABSICHT, "ZB4").expect("published");
    assert_eq!(zb4.cluster, Cluster::Zustimmung);
    assert!(zb4.bedeutung.contains("Eigenausbau"));
}

/// No MSB-Wechsel tree lets a party refuse because the counterparty is not in
/// its Marktpartnerverzeichnis. That case must escalate.
#[test]
fn no_tree_publishes_an_unknown_marktpartner_rejection() {
    for (id, list) in [
        (codes::EBD_KUENDIGUNG_MSB, codes::E_0200_CODES),
        (codes::EBD_ANMELDUNG_MSB, codes::E_0201_CODES),
        (codes::EBD_ABMELDUNG_MSB, codes::E_0202_CODES),
        (codes::EBD_VERPFLICHTUNGSANFRAGE, codes::E_0240_CODES),
    ] {
        for c in list {
            let b = c.bedeutung.to_lowercase();
            assert!(
                !b.contains("marktpartner") && !b.contains("verzeichnis"),
                "{id} publishes a Marktpartner rejection: {}",
                c.code
            );
        }
    }
}

/// `E_0202` is the narrowest of the four and the narrowness is load-bearing:
/// the NB may not refuse an Abmeldung for „keine Zuordnung möglich".
#[test]
fn the_abmeldung_tree_has_no_zuordnungs_ablehnung() {
    assert!(codes::lookup(codes::EBD_ABMELDUNG_MSB, "ZC9").is_none());
    assert_eq!(
        codes::E_0202_CODES
            .iter()
            .filter(|c| c.cluster == Cluster::Ablehnung)
            .count(),
        2
    );
}
