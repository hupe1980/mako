//! `mako-pruefung` — the MaBiS and Redispatch trees.
//!
//! These pin the properties that make a MaBiS answer *checkable*: a code is
//! resolved within its tree, the Prüfschritt order is the rule, and an unknown
//! fact escalates instead of producing a plausible code.
#![cfg(feature = "role-mabis")]

use time::macros::datetime;

use mako_pruefung::codes::Cluster;
use mako_pruefung::mabis::{
    self, Aktivierung, AusfallarbeitsZeitreihe, Deaktivierung, GegenvorschlagPruefung,
    Korrekturgrund, Korrekturposition, ListenEntscheidung, ListenPruefung, MabisEntscheidung,
    ProfilPruefung, Profilart, ZeitreihenPruefung, Zuordnung,
};

fn antwort(e: &MabisEntscheidung) -> (&str, Cluster, u16) {
    let a = e
        .antwort_ref()
        .unwrap_or_else(|| panic!("expected an answer, got {e:?}"));
    (a.code.as_str(), a.cluster, a.pruefschritt)
}

// ── Catalogue integrity ───────────────────────────────────────────────────────

#[test]
fn every_code_resolves_within_its_own_tree() {
    for (ebd, codes) in mabis::MABIS_TREES {
        assert!(!codes.is_empty(), "{ebd} publishes no codes");
        for c in *codes {
            assert_eq!(
                c.ebd,
                Some(*ebd),
                "{ebd} publishes {} but the code names {:?}",
                c.code,
                c.ebd
            );
            assert!(
                mabis::lookup(ebd, c.code).is_some(),
                "{ebd}/{} does not resolve",
                c.code
            );
        }
        let mut seen: Vec<&str> = codes.iter().map(|c| c.code).collect();
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(before, seen.len(), "{ebd} publishes a duplicate code");
    }
}

/// The numbering hole that makes a shared grund-to-code table wrong.
#[test]
fn e_0049_publishes_no_a06_but_e_0004_does() {
    assert!(mabis::lookup("E_0004", "A06").is_some());
    assert!(
        mabis::lookup("E_0049", "A06").is_none(),
        "E_0049 does not publish A06 — its Daten-Korrekturgrund is A07"
    );
    assert_eq!(
        mabis::lookup("E_0049", "A07").unwrap().bedeutung,
        mabis::lookup("E_0004", "A06").unwrap().bedeutung,
        "the same Korrekturgrund under two different codes"
    );
}

/// The BKV checks the BK-SZR for its own Bilanzkreis; a Lieferant ordinarily
/// holds that role.
#[test]
fn the_bkv_trees_are_catalogued() {
    for ebd in ["E_0063", "E_0064", "E_0098", "E_0099"] {
        assert!(mabis::zustimmung(ebd).is_some(), "{ebd}");
        assert_eq!(
            mabis::lookup(ebd, "A01").unwrap().cluster,
            Cluster::Abweisung
        );
    }
}

#[test]
fn a_code_does_not_leak_between_trees() {
    // `A02` means four unrelated things across the MaBiS trees.
    let bedeutungen: Vec<_> = ["E_0041", "E_0062", "E_0020", "E_0070"]
        .iter()
        .map(|t| mabis::lookup(t, "A02").unwrap().bedeutung)
        .collect();
    let mut uniq = bedeutungen.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(uniq.len(), bedeutungen.len(), "{bedeutungen:?}");
}

#[test]
fn an_abweisung_is_not_forwarded_but_an_ablehnung_is() {
    let abweisung = mabis::lookup("E_0041", "A01").unwrap();
    let ablehnung = mabis::lookup("E_0041", "A05").unwrap();
    assert!(abweisung.cluster.ist_abweisung());
    assert!(!ablehnung.cluster.ist_abweisung());
    // Both refuse, so agreement alone cannot tell them apart.
    assert_eq!(abweisung.ist_zustimmung(), Some(false));
    assert_eq!(ablehnung.ist_zustimmung(), Some(false));
}

#[test]
fn a_reklamation_is_off_the_agreement_axis() {
    // A Profil-Reklamation does not invalidate the profile it complains about.
    assert_eq!(
        mabis::lookup("E_0100", "A04").unwrap().ist_zustimmung(),
        None
    );
}

// ── Summenzeitreihen ──────────────────────────────────────────────────────────

fn zeitreihe<'a>(versionen: &'a [&'a str]) -> ZeitreihenPruefung<'a> {
    ZeitreihenPruefung {
        eingang: datetime!(2026-03-10 09:00 UTC),
        clearingfrist_ende: Some(datetime!(2026-03-20 23:59 UTC)),
        mabis_zp_aktiv: Some(true),
        version: Some("20260310090000000"),
        bekannte_versionen: versionen,
        energiemengen_plausibel: Some(true),
    }
}

#[test]
fn a_clean_zeitreihe_is_accepted() {
    let p = zeitreihe(&[]);
    assert_eq!(antwort(&mabis::pruefe_zeitreihe("E_0041", &p)).0, "A06");
}

/// The order is the rule: refused before assessed beats implausible.
#[test]
fn a_late_zeitreihe_is_abgewiesen_even_when_the_figures_are_wrong() {
    let mut p = zeitreihe(&[]);
    p.eingang = datetime!(2026-03-21 09:00 UTC);
    p.energiemengen_plausibel = Some(false);
    let e = mabis::pruefe_zeitreihe("E_0041", &p);
    assert_eq!(antwort(&e), ("A01", Cluster::Abweisung, 1));
}

#[test]
fn a_repeated_version_is_a_dublette_and_a_lower_one_is_not_permitted() {
    let mut p = zeitreihe(&["20260310090000000"]);
    assert_eq!(antwort(&mabis::pruefe_zeitreihe("E_0007", &p)).0, "A03");

    let hoeher = ["20260311090000000"];
    p.bekannte_versionen = &hoeher;
    assert_eq!(antwort(&mabis::pruefe_zeitreihe("E_0007", &p)).0, "A04");
}

#[test]
fn an_unknown_fact_escalates_instead_of_guessing() {
    let mut p = zeitreihe(&[]);
    p.energiemengen_plausibel = None;
    let e = mabis::pruefe_zeitreihe("E_0093", &p);
    match e {
        MabisEntscheidung::Eskalation { pruefschritt, .. } => assert_eq!(pruefschritt, 5),
        other => panic!("expected an escalation, got {other:?}"),
    }
}

#[test]
fn the_short_form_trees_have_their_own_numbering() {
    // „Zeitreihe akzeptiert" is A06 in the long form and A03/A04 in the short.
    for ebd in ["E_0062", "E_0063", "E_0064", "E_0098", "E_0099"] {
        let e = mabis::pruefe_zeitreihe_kurzform(ebd, false, Some(true));
        assert_eq!(antwort(&e).0, "A03", "{ebd}");
    }
    assert_eq!(
        antwort(&mabis::pruefe_dzue(false, true, Some(true))).0,
        "A04"
    );
    // Without its Liste the DZÜ is not assessable at all.
    assert_eq!(
        antwort(&mabis::pruefe_dzue(false, false, Some(true))).0,
        "A02"
    );
}

// ── Listenabgleich ────────────────────────────────────────────────────────────

#[test]
fn a_whole_list_refusal_carries_no_positions() {
    let positionen = [Korrekturposition {
        malo: "51238696781".into(),
        grund: Korrekturgrund::Entfallen,
    }];
    let p = ListenPruefung {
        abonnement_bestellt: Some(false),
        version_zugelassen: Some(true),
        positionen: &positionen,
        ..ListenPruefung::default()
    };
    let e = mabis::pruefe_liste("E_0004", &p);
    assert_eq!(e.korrekturen(), 0, "a refused list states no positions");
    match e {
        ListenEntscheidung::GesamtAblehnung(a) => {
            assert_eq!(a.code, "A01");
            assert_eq!(a.cluster, Cluster::AblehnungDerGesamtenListe);
            assert!(!a.traegt_positionen());
        }
        other => panic!("expected a whole-list refusal, got {other:?}"),
    }
}

#[test]
fn an_empty_korrekturliste_is_still_an_answer() {
    let p = ListenPruefung {
        abonnement_bestellt: Some(true),
        version_zugelassen: Some(true),
        positionen: &[],
        ..ListenPruefung::default()
    };
    let e = mabis::pruefe_liste("E_0004", &p);
    assert!(e.ist_korrekturliste(), "silence would read as acceptance");
    assert_eq!(e.korrekturen(), 0);
}

#[test]
fn the_same_grund_gets_a_different_code_in_each_tree() {
    let positionen = [Korrekturposition {
        malo: "51238696781".into(),
        grund: Korrekturgrund::DatenFehlerhaft,
    }];
    let abo = ListenPruefung {
        abonnement_bestellt: Some(true),
        version_zugelassen: Some(true),
        positionen: &positionen,
        ..ListenPruefung::default()
    };
    let einzel = ListenPruefung {
        zeitraum_plausibel: Some(true),
        mabis_zp_passt: Some(true),
        version_zugelassen: Some(true),
        positionen: &positionen,
        ..ListenPruefung::default()
    };
    let code = |e: ListenEntscheidung| match e {
        ListenEntscheidung::Korrekturliste(l) => l[0].antwort.code.clone(),
        other => panic!("expected a Korrekturliste, got {other:?}"),
    };
    assert_eq!(code(mabis::pruefe_liste("E_0004", &abo)), "A06");
    assert_eq!(code(mabis::pruefe_liste("E_0049", &abo)), "A07");
    assert_eq!(code(mabis::pruefe_liste("E_0014", &einzel)), "A07");
}

#[test]
fn a_grund_a_tree_does_not_publish_escalates() {
    let positionen = [Korrekturposition {
        malo: "51238696781".into(),
        grund: Korrekturgrund::Ergaenzt,
    }];
    let p = ListenPruefung {
        innerhalb_clearingphase: Some(true),
        positionen: &positionen,
        ..ListenPruefung::default()
    };
    // The DZÜ-Liste publishes only „nicht bekannt" and „Daten fehlerhaft".
    match mabis::pruefe_liste("E_0070", &p) {
        ListenEntscheidung::Eskalation { grund, .. } => assert!(grund.contains("E_0070")),
        other => panic!("expected an escalation, got {other:?}"),
    }
}

// ── MaBiS-Zählpunkt ───────────────────────────────────────────────────────────

fn aktivierung_ok() -> Aktivierung {
    Aktivierung {
        frist_gewahrt: Some(true),
        zeitpunkt_zulaessig: Some(true),
        id_frei: Some(true),
        bilanzierungsgebiet_gueltig: Some(true),
        regelzone_korrekt: Some(true),
        berechtigt: Some(true),
        kein_abweichender_zp: Some(true),
        keine_abweichende_id: Some(true),
        zrt_berechtigt: Some(true),
        obis_passend: Some(true),
        nicht_bereits_aktiv: Some(true),
    }
}

#[test]
fn the_aktivierung_walks_eleven_gates_in_order() {
    assert_eq!(
        antwort(&mabis::pruefe_aktivierung(&aktivierung_ok())).0,
        "A12"
    );

    let mut a = aktivierung_ok();
    a.regelzone_korrekt = Some(false);
    a.obis_passend = Some(false);
    // The earlier gate wins.
    let e = mabis::pruefe_aktivierung(&a);
    assert_eq!(antwort(&e), ("A05", Cluster::Ablehnung, 5));
}

/// An Aktivierung that misses its Frist has still been assessed.
#[test]
fn zp_lifecycle_refusals_are_ablehnungen_not_abweisungen() {
    let mut a = aktivierung_ok();
    a.frist_gewahrt = Some(false);
    let e = mabis::pruefe_aktivierung(&a);
    let (_, cluster, _) = antwort(&e);
    assert_eq!(cluster, Cluster::Ablehnung);
    assert!(!cluster.ist_abweisung(), "its Prüfmitteilung is forwarded");
}

#[test]
fn a_deaktivierung_with_zeitreihen_is_refused() {
    let d = Deaktivierung {
        frist_gewahrt: Some(true),
        zeitpunkt_zulaessig: Some(true),
        id_frei: Some(true),
        nicht_bereits_deaktiviert: Some(true),
        keine_zeitreihen_vorhanden: Some(false),
    };
    assert_eq!(antwort(&mabis::pruefe_deaktivierung(&d)).0, "A05");
}

#[test]
fn sonstiges_produces_a99_and_carries_its_erlaeuterung() {
    let z = Zuordnung {
        id_frei: Some(true),
        passt_zur_vereinbarung: Some(true),
        berechtigt: Some(true),
        beteiligt: Some(true),
        zuordnungslage_ok: Some(true),
        sonstiges: Some("Netzzeitreihe wird umgebaut".into()),
    };
    let e = mabis::pruefe_zuordnung(&z);
    let a = e.antwort_ref().unwrap();
    assert_eq!(a.code, "A99");
    assert_eq!(a.bemerkung.as_deref(), Some("Netzzeitreihe wird umgebaut"));

    let mut sauber = z.clone();
    sauber.sonstiges = None;
    assert_eq!(antwort(&mabis::pruefe_zuordnung(&sauber)).0, "A06");
}

/// `E_0103` asks four questions where `E_0102` asks five, so the codes shift.
#[test]
fn the_beendigung_tree_does_not_ask_about_the_id() {
    let z = Zuordnung {
        id_frei: Some(false), // would be A01 in E_0102 — not asked here
        passt_zur_vereinbarung: Some(true),
        berechtigt: Some(true),
        beteiligt: Some(true),
        zuordnungslage_ok: Some(true),
        sonstiges: None,
    };
    assert_eq!(antwort(&mabis::pruefe_beendigung_zuordnung(&z)).0, "A05");
    assert_eq!(antwort(&mabis::pruefe_zuordnung(&z)).0, "A01");
}

// ── Profile ───────────────────────────────────────────────────────────────────

fn profil(art: Profilart) -> ProfilPruefung {
    ProfilPruefung {
        art,
        abonniert: Some(true),
        version_hoeher: Some(true),
        masseinheit_passt: Some(true),
        niedrigste_temperaturmasszahl_passt: Some(true),
        anzahl_temperaturmasszahlen_passt: Some(true),
    }
}

#[test]
fn an_acceptable_profile_is_answered_with_silence() {
    assert_eq!(
        mabis::pruefe_profil(&profil(Profilart::Profil)),
        MabisEntscheidung::Schweigen
    );
}

/// A Profil has no Temperaturmaßzahlen, so it can never reach A04–A06.
#[test]
fn the_profilschar_only_steps_are_not_run_on_a_profil() {
    let mut p = profil(Profilart::Profil);
    p.masseinheit_passt = Some(false);
    p.anzahl_temperaturmasszahlen_passt = Some(false);
    assert_eq!(mabis::pruefe_profil(&p), MabisEntscheidung::Schweigen);

    let mut schar = profil(Profilart::Profilschar);
    schar.masseinheit_passt = Some(false);
    let e = mabis::pruefe_profil(&schar);
    assert_eq!(antwort(&e), ("A04", Cluster::Reklamation, 5));
}

#[test]
fn a_stale_version_is_a02_for_a_profil_and_a03_for_a_profilschar() {
    let mut p = profil(Profilart::Profil);
    p.version_hoeher = Some(false);
    let e = mabis::pruefe_profil(&p);
    assert_eq!(antwort(&e), ("A02", Cluster::Reklamation, 3));

    let mut schar = profil(Profilart::Profilschar);
    schar.version_hoeher = Some(false);
    let e2 = mabis::pruefe_profil(&schar);
    assert_eq!(antwort(&e2), ("A03", Cluster::Reklamation, 4));
}

// ── Redispatch-Ausfallarbeit ──────────────────────────────────────────────────

/// BDEW states the two runs can reach different results.
#[test]
fn e_0902_is_decided_per_series() {
    let ausfallarbeit = mabis::pruefe_ausfallarbeit(
        AusfallarbeitsZeitreihe::Ausfallarbeit,
        Some(true),
        None,
        None,
    );
    let fahrplan = mabis::pruefe_ausfallarbeit(
        AusfallarbeitsZeitreihe::Fahrplananteil,
        Some(false),
        Some(true),
        Some("Fahrplananteil weicht um 12 % ab".into()),
    );
    assert_eq!(antwort(&ausfallarbeit).0, "A01");
    assert_eq!(antwort(&fahrplan).0, "A02");
}

/// The two Ablehnungen differ in what the NB owes next, not in why it refused.
#[test]
fn gegenvorschlag_and_korrekturanforderung_are_different_codes() {
    let mit = mabis::pruefe_ausfallarbeit(
        AusfallarbeitsZeitreihe::Ausfallarbeit,
        Some(false),
        Some(true),
        Some("begründet".into()),
    );
    let ohne = mabis::pruefe_ausfallarbeit(
        AusfallarbeitsZeitreihe::Ausfallarbeit,
        Some(false),
        Some(false),
        Some("begründet".into()),
    );
    assert_eq!(antwort(&mit).0, "A02");
    assert_eq!(antwort(&ohne).0, "A03");
    for e in [&mit, &ohne] {
        assert!(e.antwort_ref().unwrap().bemerkung.is_some());
    }
}

#[test]
fn only_one_gegenvorschlag_is_admissible() {
    let g = GegenvorschlagPruefung {
        noch_keine_zustimmung: Some(true),
        frist_gewahrt: Some(true),
        kein_frueherer_gegenvorschlag: Some(false),
        energiemengen_plausibel: Some(true),
    };
    assert_eq!(antwort(&mabis::pruefe_gegenvorschlag(&g)).0, "A03");
}

#[test]
fn a_settled_ausfallarbeitszeitreihe_takes_no_gegenvorschlag() {
    let g = GegenvorschlagPruefung {
        noch_keine_zustimmung: Some(false),
        ..GegenvorschlagPruefung::default()
    };
    let e = mabis::pruefe_gegenvorschlag(&g);
    assert_eq!(antwort(&e), ("A01", Cluster::Ablehnung, 1));
}

// ── Einzelanforderung ─────────────────────────────────────────────────────────

#[test]
fn a_marktlokation_without_a_lieferant_is_answered_not_delivered() {
    assert_eq!(
        antwort(&mabis::pruefe_lieferantenzuordnung("E_0068", Some(false))).0,
        "A01"
    );
    assert_eq!(
        mabis::pruefe_lieferantenzuordnung("E_0104", Some(true)),
        MabisEntscheidung::Schweigen
    );
}
/// The Zustimmung codes quoted in the READMEs and in
/// `concepts/MABIS_REDISPATCH.md`. They differ per tree, so a doc that names
/// one is a claim about the catalogue and is pinned here.
#[test]
fn documented_zustimmung_codes_match_the_catalogue() {
    for (ebd, expect) in [
        ("E_0010", Some("A06")),
        ("E_0020", Some("A12")),
        ("E_0102", Some("A06")),
        ("E_0103", Some("A05")),
        ("E_0902", Some("A01")),
        ("E_0901", Some("A05")),
        ("E_0065", Some("A04")),
        ("E_0062", Some("A03")),
        ("E_0100", None),
        ("E_0070", None),
        ("E_0004", None),
    ] {
        assert_eq!(
            mako_pruefung::mabis::zustimmung(ebd).map(|c| c.code),
            expect,
            "{ebd}"
        );
    }
}
