//! The LF Entscheidungsbäume, walked against the published Prüfschritte.
//!
//! Each test names the EBD and the Prüfschritt it pins, so a future edit that
//! moves a landing has to argue with the document rather than with a number.
//!
//! Source: BDEW *Entscheidungsbaum-Diagramme und Codelisten* 4.3 (01.04.2026).

#![cfg(feature = "role-lf")]

use time::macros::{date, datetime};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use mako_pruefung::codes::{
    E_0609_CODES, E_0614_CODES, E_0624_CODES, E_3001_CODES, E_3002_CODES, E_3020_CODES, lookup,
};
use mako_pruefung::{
    Bekannt, LfAnfrage, LfEntscheidung, LfVertragslage, Lokationsart, Terminart, Vollmacht,
    pruefe_abmeldung, pruefe_abmeldung_gas, pruefe_abmeldungsanfrage_gas,
    pruefe_beendigung_zuordnung, pruefe_kuendigung, pruefe_kuendigung_gas,
};

const TERMIN: Date = date!(2026 - 09 - 01);
const EINGANG: OffsetDateTime = datetime!(2026-08-20 09:00 UTC);

fn anfrage(pid: u32, grund: Option<&str>) -> LfAnfrage {
    LfAnfrage {
        pid,
        process_id: Uuid::nil(),
        malo_id: "51238696012".to_owned(),
        vorgangsnummer: Some("VORGANG-0001".to_owned()),
        absender_mp_id: "9900357000004".to_owned(),
        empfaenger_mp_id: "9900000000001".to_owned(),
        lokationsart: Some(Lokationsart::VerbrauchendeMalo),
        transaktionsgrund: grund.map(ToOwned::to_owned),
        termin: Some(TERMIN),
        terminart: Terminart::Fix,
        // `SG4 DTM+154` is Muss on a 55010 and `E_0624` Prüfschritt 5 measures
        // its Frist from it; the day before `EINGANG` is inside the window.
        uet_lieferanmeldung: (pid == 55_010).then_some(date!(2026 - 08 - 20)),
        eingang: EINGANG,
    }
}

/// A supply state where every question the trees ask has an answer, so a test
/// only has to set the field it is about.
fn lage() -> LfVertragslage {
    LfVertragslage {
        beliefert: true,
        vertrag_vorhanden: Bekannt::Ja,
        // `E_0614` Prüfschritt 70 is „kündbar zum übermittelten Termin?", and
        // the answer is this date against the requested one — a contract
        // terminable to the day before TERMIN is terminable to TERMIN.
        naechstmoeglicher_kuendigungstermin: Some(date!(2026 - 08 - 31)),
        zuordnung_am_folgetag: Bekannt::Ja,
        vertragsbindung_am_folgetag: Bekannt::Nein,
        kunde_identisch: Bekannt::Nein,
        kunde_nicht_ausgezogen: Bekannt::Nein,
        in_ersatzversorgung_am_folgetag: Bekannt::Nein,
        keine_stilllegung: Bekannt::Nein,
        zrt_wechsel_mit_ermaechtigung: Bekannt::Nein,
        zuordnungsermaechtigung_deaktiviert: Bekannt::Ja,
        vorlauffrist_eingehalten: Bekannt::Ja,
        ..LfVertragslage::default()
    }
}

fn code_of(e: &LfEntscheidung) -> &str {
    e.as_antwort()
        .unwrap_or_else(|| panic!("expected an answer, got {e:?}"))
        .code
        .as_str()
}

// ── E_0609 — Abmeldung prüfen (55007) ─────────────────────────────────────────

/// Prüfschritt 130 → `A10` „Lieferende wird zugestimmt".
#[test]
fn e0609_ordinary_abmeldung_is_a10() {
    let e = pruefe_abmeldung(&anfrage(55_007, Some("Z33")), &lage());
    assert_eq!(code_of(&e), "A10");
    assert!(e.ist_zustimmung());
    assert_eq!(e.as_antwort().unwrap().ebd.as_deref(), Some("E_0609"));
}

/// Prüfschritt 30 → `A02` „Lieferende zum Abmeldedatum wurde bereits bestätigt".
#[test]
fn e0609_already_confirmed_end_is_a02() {
    let mut l = lage();
    l.bestaetigtes_zuordnungsende = Some(TERMIN);
    let e = pruefe_abmeldung(&anfrage(55_007, Some("Z33")), &l);
    assert_eq!(code_of(&e), "A02");
    assert!(!e.ist_zustimmung());
}

/// Prüfschritt 40 → `A03` „Vorlauffrist wurde nicht eingehalten".
#[test]
fn e0609_missed_vorlauffrist_is_a03() {
    let mut l = lage();
    l.vorlauffrist_eingehalten = Bekannt::Nein;
    assert_eq!(
        code_of(&pruefe_abmeldung(&anfrage(55_007, Some("Z33")), &l)),
        "A03"
    );
}

/// An unevaluated Vorlauffrist is not a passing one. The tree stops.
#[test]
fn e0609_unknown_vorlauffrist_escalates() {
    let mut l = lage();
    l.vorlauffrist_eingehalten = Bekannt::Unbekannt;
    let e = pruefe_abmeldung(&anfrage(55_007, Some("Z33")), &l);
    assert!(e.ist_eskalation(), "{e:?}");
}

/// Prüfschritt 60 → `A04`: the supplier holds information that the MaLo is not
/// being decommissioned, and the BDEW requires that information in writing.
#[test]
fn e0609_contested_stilllegung_is_a04_with_a_bemerkung() {
    let mut l = lage();
    l.keine_stilllegung = Bekannt::Ja;
    let e = pruefe_abmeldung(&anfrage(55_007, Some("Z33")), &l);
    let a = e.as_antwort().expect("answer");
    assert_eq!(a.code, "A04");
    assert!(
        a.bemerkung.is_some(),
        "A04 requires an Erläuterung (FTX+ACB)"
    );
}

/// Prüfschritt 85 → `A05`: on a BKV deactivation the Lieferende must fall on a
/// Monatserster.
#[test]
fn e0609_bkv_deactivation_off_month_start_is_a05() {
    let mut a = anfrage(55_007, Some("ZQ7"));
    a.termin = Some(date!(2026 - 09 - 15));
    assert_eq!(code_of(&pruefe_abmeldung(&a, &lage())), "A05");
}

/// Prüfschritt 100 → `A07`: the BKV has not deactivated the
/// Zuordnungsermächtigung from the supplier's point of view.
#[test]
fn e0609_ermaechtigung_not_deactivated_is_a07() {
    let mut l = lage();
    l.zuordnungsermaechtigung_deaktiviert = Bekannt::Nein;
    assert_eq!(
        code_of(&pruefe_abmeldung(&anfrage(55_007, Some("ZQ7")), &l)),
        "A07"
    );
}

/// A Tranche walks the same questions and lands in the `A21`–`A29` range.
#[test]
fn e0609_tranche_uses_the_second_code_range() {
    let mut a = anfrage(55_007, Some("Z33"));
    a.lokationsart = Some(Lokationsart::Tranche);
    assert_eq!(code_of(&pruefe_abmeldung(&a, &lage())), "A29");

    let mut l = lage();
    l.vorlauffrist_eingehalten = Bekannt::Nein;
    assert_eq!(code_of(&pruefe_abmeldung(&a, &l)), "A22");
}

/// `ZQ7`'s Vorlauffrist hangs on a Deaktivierungsmeldung the supplier never
/// sees, so Prüfschritt 40 has nothing to evaluate and `E_0609` gives the Grund
/// its own Frist at 120. The walk therefore reaches 85, 100 and 120.
#[test]
fn e0609_the_zq7_branch_walks_past_pruefschritt_40() {
    let mut a = anfrage(55_007, Some("ZQ7"));
    // Prüfschritt 85 — das Lieferende muss der nächste Monatserste sein.
    a.termin = Some(date!(2026 - 09 - 01)); // EINGANG is 2026-08-20
    let mut l = lage();
    l.vorlauffrist_eingehalten = Bekannt::Ja;
    l.zuordnungsermaechtigung_deaktiviert = Bekannt::Ja;
    assert_eq!(code_of(&pruefe_abmeldung(&a, &l)), "A10");

    // 100 → A07: the LF has no record of the BKV deactivating anything.
    l.zuordnungsermaechtigung_deaktiviert = Bekannt::Nein;
    assert_eq!(code_of(&pruefe_abmeldung(&a, &l)), "A07");
}

/// Prüfschritt 85 asks for the **next** Monatserster, not merely a Monatserster:
/// a first of the month two years out claims to end a Zuordnungsermächtigung
/// that has nothing to do with it.
#[test]
fn e0609_a_distant_monatserster_is_a05() {
    let mut a = anfrage(55_007, Some("ZQ7"));
    a.termin = Some(date!(2028 - 01 - 01));
    let mut l = lage();
    l.vorlauffrist_eingehalten = Bekannt::Ja;
    l.zuordnungsermaechtigung_deaktiviert = Bekannt::Ja;
    assert_eq!(code_of(&pruefe_abmeldung(&a, &l)), "A05");

    // …and a mid-month date is the same refusal.
    a.termin = Some(date!(2026 - 09 - 15));
    assert_eq!(code_of(&pruefe_abmeldung(&a, &l)), "A05");
}

/// `A32` and `A35` belong to `E_0624`. `E_0609` must never produce them: a
/// code is only meaningful inside the tree that publishes it.
#[test]
fn e0609_never_produces_an_e0624_code() {
    let gruende = [
        None,
        Some("E01"),
        Some("E03"),
        Some("Z33"),
        Some("ZQ7"),
        Some("ZT0"),
    ];
    let arten = [
        Lokationsart::VerbrauchendeMalo,
        Lokationsart::ErzeugendeMalo,
        Lokationsart::Tranche,
    ];
    for grund in gruende {
        for art in arten {
            for bindung in [Bekannt::Ja, Bekannt::Nein, Bekannt::Unbekannt] {
                let mut a = anfrage(55_007, grund);
                a.lokationsart = Some(art);
                let mut l = lage();
                l.vertragsbindung_am_folgetag = bindung;
                if let Some(code) = pruefe_abmeldung(&a, &l).as_antwort() {
                    assert!(
                        !matches!(code.code.as_str(), "A32" | "A35"),
                        "E_0609 produced the E_0624 code {} for {grund:?}/{art:?}",
                        code.code
                    );
                    assert!(
                        lookup("E_0609", &code.code).is_some(),
                        "{} is not published by E_0609",
                        code.code
                    );
                }
            }
        }
    }
}

// ── E_0624 — Anfrage zur Beendigung der Zuordnung (55010) ─────────────────────

/// Prüfschritt 90 → `A36` „Vertragsverhältnis wurde zum angefragten oder davor
/// liegenden Termin beendet".
#[test]
fn e0624_contract_already_over_is_a36() {
    let e = pruefe_beendigung_zuordnung(&anfrage(55_010, Some("E03")), &lage());
    assert_eq!(code_of(&e), "A36");
    assert!(e.ist_zustimmung());
}

/// Prüfschritt 90 → `A35` „Es besteht eine Vertragsbindung".
#[test]
fn e0624_running_contract_is_a35() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    let e = pruefe_beendigung_zuordnung(&anfrage(55_010, Some("E03")), &l);
    assert_eq!(code_of(&e), "A35");
    assert!(!e.ist_zustimmung());
}

/// Prüfschritt 50 → `A32`. Note the direction: the LFA refuses **because it is
/// not an Einzug** — the customer is the same one it already has.
#[test]
fn e0624_same_customer_means_it_is_not_an_einzug() {
    let mut l = lage();
    l.kunde_identisch = Bekannt::Ja;
    let e = pruefe_beendigung_zuordnung(&anfrage(55_010, Some("E01")), &l);
    assert_eq!(code_of(&e), "A32");
    assert!(!e.ist_zustimmung());
    assert!(
        e.as_antwort()
            .unwrap()
            .bedeutung
            .contains("nicht um einen Einzug"),
        "A32 means the LFA denies the Einzug"
    );
}

/// Prüfschritt 60 → `A34`: a genuine move-out. The answer must carry the LFA's
/// **own** Lieferendedatum, which the EBD states explicitly.
#[test]
fn e0624_genuine_einzug_is_a34_with_the_lfa_date() {
    let mut l = lage();
    l.vertragsende = Some(date!(2026 - 08 - 31));
    let e = pruefe_beendigung_zuordnung(&anfrage(55_010, Some("E01")), &l);
    let a = e.as_antwort().expect("answer");
    assert_eq!(a.code, "A34");
    assert_eq!(a.termin, Some(date!(2026 - 08 - 31)));
}

/// Prüfschritt 80 → `A38`: an LFA that is also Grundversorger ends the
/// Ersatzversorgung rather than claiming a Vertragsbindung.
#[test]
fn e0624_grundversorger_ending_ersatzversorgung_is_a38() {
    let mut l = lage();
    l.ist_grundversorger = true;
    l.in_ersatzversorgung_am_folgetag = Bekannt::Ja;
    l.vertragsbindung_am_folgetag = Bekannt::Ja; // would otherwise be A35
    assert_eq!(
        code_of(&pruefe_beendigung_zuordnung(
            &anfrage(55_010, Some("E03")),
            &l
        )),
        "A38"
    );
}

/// Prüfschritt 5 → `A43`: the request itself was late. This is the *first*
/// question the tree asks, before the Vorgang is even classified.
#[test]
fn e0624_late_request_is_a43() {
    let mut a = anfrage(55_010, Some("E03"));
    // ÜT was a Monday; the Frist ran out at 07:00 on Tuesday.
    a.uet_lieferanmeldung = Some(date!(2026 - 08 - 17));
    a.eingang = datetime!(2026-08-19 09:00 UTC);
    assert_eq!(code_of(&pruefe_beendigung_zuordnung(&a, &lage())), "A43");
}

/// The same request inside the window walks on.
#[test]
fn e0624_request_inside_the_window_is_not_a43() {
    let mut a = anfrage(55_010, Some("E03"));
    a.uet_lieferanmeldung = Some(date!(2026 - 08 - 17));
    a.eingang = datetime!(2026-08-18 04:00 UTC); // 06:00 Berlin, before 07:00
    assert_ne!(code_of(&pruefe_beendigung_zuordnung(&a, &lage())), "A43");
}

/// Prüfschritt 30 → `A30`: the supply already ended and the NB confirmed it.
#[test]
fn e0624_already_ended_and_confirmed_is_a30() {
    let mut l = lage();
    l.zuordnung_am_folgetag = Bekannt::Nein;
    l.bestaetigtes_zuordnungsende = Some(TERMIN);
    assert_eq!(
        code_of(&pruefe_beendigung_zuordnung(&anfrage(55_010, None), &l)),
        "A30"
    );
}

/// An unknown supply state stops the walk at Prüfschritt 20 instead of guessing.
#[test]
fn e0624_unknown_supply_state_escalates() {
    let mut l = lage();
    l.zuordnung_am_folgetag = Bekannt::Unbekannt;
    let e = pruefe_beendigung_zuordnung(&anfrage(55_010, None), &l);
    assert!(e.ist_eskalation(), "{e:?}");
    match e {
        LfEntscheidung::Eskalation { pruefschritt, .. } => assert_eq!(pruefschritt, 20),
        LfEntscheidung::Antwort(_) => unreachable!(),
    }
}

// ── E_0614 — Kündigung Vertrag prüfen (55016) ─────────────────────────────────

/// Prüfschritt 120 → `A09` „Zustimmung".
#[test]
fn e0614_ordinary_kuendigung_is_a09() {
    let e = pruefe_kuendigung(&anfrage(55_016, None), &lage());
    assert_eq!(code_of(&e), "A09");
    assert!(e.ist_zustimmung());
}

/// Prüfschritt 20 → `A01`: the Kündigungstermin is already in the past.
#[test]
fn e0614_backdated_termin_is_a01() {
    let mut a = anfrage(55_016, None);
    a.termin = Some(date!(2026 - 08 - 01));
    assert_eq!(code_of(&pruefe_kuendigung(&a, &lage())), "A01");
}

/// Prüfschritt 40 → `A03`, and it is a **Zustimmung**: the contract is already
/// terminated to exactly that date, so the LFA confirms rather than refuses.
#[test]
fn e0614_already_terminated_to_that_date_is_a_zustimmung() {
    let mut l = lage();
    l.vertragsende = Some(TERMIN);
    let e = pruefe_kuendigung(&anfrage(55_016, None), &l);
    assert_eq!(code_of(&e), "A03");
    assert!(
        e.ist_zustimmung(),
        "A03 sits in the Zustimmungs-Cluster and must ride the Bestätigungs-PID"
    );
}

/// Prüfschritt 80 → `A06` „Vertragsbindung".
#[test]
fn e0614_running_contract_is_a06() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    // `A06`'s 55018 carries `SG4 DTM+157` — „der Zeitpunkt, zu welchem der
    // Vertrag am Tag des Versandes der Antwort noch kündbar ist".
    l.naechstmoeglicher_kuendigungstermin = Some(date!(2026 - 12 - 31));
    let e = pruefe_kuendigung(&anfrage(55_016, None), &l);
    assert_eq!(code_of(&e), "A06");
    assert_eq!(
        e.as_antwort().unwrap().termin,
        Some(date!(2026 - 12 - 31)),
        "the Ablehnung must name the date the contract is still terminable to"
    );
}

/// Prüfschritt 60 is the split that decides whether `A05`/`A06` are reachable
/// at all. A Kündigung „zum nächstmöglichen Termin" (`SG4 DTM+471`) is the
/// ordinary LFW24 case, and the EBD sends it straight past the Kündbarkeits-
/// frage to the Zustimmung — refusing it for Vertragsbindung would block every
/// switch of a customer who is still inside a Laufzeitvertrag.
#[test]
fn e0614_kuendigung_zum_naechstmoeglichen_termin_is_confirmed() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    l.naechstmoeglicher_kuendigungstermin = Some(date!(2026 - 12 - 31));

    let mut a = anfrage(55_016, None);
    a.terminart = Terminart::Naechstmoeglich;

    let e = pruefe_kuendigung(&a, &l);
    assert_eq!(code_of(&e), "A09");
    assert!(e.ist_zustimmung());
    assert_eq!(
        e.as_antwort().unwrap().termin,
        Some(date!(2026 - 12 - 31)),
        "the Bestätigung states the date the LFA determined (DTM+471, [513])"
    );
}

/// The same message with a fixed date is the one that may be refused.
#[test]
fn e0614_a_fixed_date_inside_the_binding_is_still_a06() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    l.naechstmoeglicher_kuendigungstermin = Some(date!(2026 - 12 - 31));
    let e = pruefe_kuendigung(&anfrage(55_016, None), &l);
    assert!(!e.ist_zustimmung());
    assert_eq!(code_of(&e), "A06");
}

/// `A05` is „Vertragsbindung bei bereits in der Zukunft **beendetem** Vertrag"
/// and its 55018 carries the already confirmed Kündigungsdatum. A contract
/// nobody has terminated is `A06`, whatever its next admissible date.
#[test]
fn e0614_a05_needs_an_actual_future_termination() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    l.naechstmoeglicher_kuendigungstermin = Some(date!(2027 - 06 - 30));
    assert_eq!(
        code_of(&pruefe_kuendigung(&anfrage(55_016, None), &l)),
        "A06",
        "a merely bound contract is not one that was terminated to a later date"
    );

    l.vertragsende = Some(date!(2027 - 06 - 30));
    let e = pruefe_kuendigung(&anfrage(55_016, None), &l);
    assert_eq!(code_of(&e), "A05");
    assert_eq!(e.as_antwort().unwrap().termin, Some(date!(2027 - 06 - 30)));
}

/// Prüfschritt 70 asks whether the contract is „unter Einhaltung der
/// Kündigungsfrist" terminable to the *submitted* date — not whether it is
/// running. Every unterminated contract is running, and almost all of them are
/// terminable to a date far enough out.
#[test]
fn e0614_a_terminable_fixed_date_is_confirmed_even_while_the_contract_runs() {
    let mut l = lage();
    // A running, unterminated contract: nothing has been cancelled, and the
    // Vertragsverhältnis outlives the requested date.
    l.vertragsende = None;
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    // …but the notice period is met: the next admissible date is on or before
    // the one the LFN asked for.
    l.naechstmoeglicher_kuendigungstermin = Some(TERMIN);
    let e = pruefe_kuendigung(&anfrage(55_016, None), &l);
    assert_eq!(code_of(&e), "A09", "a terminable Kündigung is confirmed");
    assert!(e.ist_zustimmung());
}

/// `E_3001` makes the same comparison: `E15` „Zustimmung ohne Korrekturen"
/// when the requested date honours the notice period, `Z12`/`Z01` only when it
/// does not.
#[test]
fn e3001_a_terminable_fixed_date_is_e15_not_z12() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    l.naechstmoeglicher_kuendigungstermin = Some(TERMIN);
    let e = pruefe_kuendigung_gas(&anfrage(44_016, None), &l);
    assert_eq!(code_of(&e), "E15");
    assert!(e.ist_zustimmung());
}

/// Prüfschritt 70 without a next admissible date is not „kündbar": the step
/// cannot be evaluated at all, and `A06`/`A15` would have no `DTM+157` to carry.
#[test]
fn e0614_an_unknown_kuendbarkeit_escalates_at_70() {
    let mut l = lage();
    l.naechstmoeglicher_kuendigungstermin = None;
    let e = pruefe_kuendigung(&anfrage(55_016, None), &l);
    assert!(e.ist_eskalation(), "{e:?}");
    match e {
        LfEntscheidung::Eskalation { pruefschritt, .. } => assert_eq!(pruefschritt, 70),
        LfEntscheidung::Antwort(_) => unreachable!(),
    }
}

/// Prüfschritt 500 → `A18` is a **record** that no contract exists, not the
/// absence of a record. A deployment that cannot look one up escalates; reading
/// „nothing found" as `A18` releases every Tranche on request.
#[test]
fn e0614_a18_needs_a_recorded_absence_not_a_missing_record() {
    let mut a = anfrage(55_016, None);
    a.lokationsart = Some(Lokationsart::Tranche);

    let mut l = lage();
    l.vertrag_vorhanden = Bekannt::Unbekannt;
    let e = pruefe_kuendigung(&a, &l);
    assert!(e.ist_eskalation(), "{e:?}");
    match e {
        LfEntscheidung::Eskalation { pruefschritt, .. } => assert_eq!(pruefschritt, 500),
        LfEntscheidung::Antwort(_) => unreachable!(),
    }

    l.vertrag_vorhanden = Bekannt::Nein;
    assert_eq!(code_of(&pruefe_kuendigung(&a, &l)), "A18");

    l.vertrag_vorhanden = Bekannt::Ja;
    assert_eq!(code_of(&pruefe_kuendigung(&a, &l)), "A17");
}

/// `E_0624` Prüfschritt 5 is the tree's *first* question and its anchor is
/// `SG4 DTM+154`. A 55010 without one cannot be measured, so it escalates —
/// skipping the step accepts every late Anfrage, which is the one thing `A43`
/// exists to refuse.
#[test]
fn e0624_a_request_without_its_uet_escalates_at_5() {
    let mut a = anfrage(55_010, Some("E03"));
    a.uet_lieferanmeldung = None;
    let e = pruefe_beendigung_zuordnung(&a, &lage());
    assert!(e.ist_eskalation(), "{e:?}");
    match e {
        LfEntscheidung::Eskalation { pruefschritt, .. } => assert_eq!(pruefschritt, 5),
        LfEntscheidung::Antwort(_) => unreachable!(),
    }
}

/// Prüfschritt 100: the EBD parks the process while an requested Vollmacht is
/// outstanding — „wartet an diesem Prüfschritt". Parking is an operator state,
/// not an answer.
#[test]
fn e0614_pending_vollmacht_parks_rather_than_answering() {
    let mut l = lage();
    l.vollmacht = Vollmacht::AngefordertAusstehend;
    let e = pruefe_kuendigung(&anfrage(55_016, None), &l);
    assert!(e.ist_eskalation(), "{e:?}");
}

/// Prüfschritt 110 → `A08`.
#[test]
fn e0614_rejected_vollmacht_is_a08() {
    let mut l = lage();
    l.vollmacht = Vollmacht::Unwirksam;
    assert_eq!(
        code_of(&pruefe_kuendigung(&anfrage(55_016, None), &l)),
        "A08"
    );
}

// ── Gas ───────────────────────────────────────────────────────────────────────

/// `E_3002` — the Gas Zustimmung is `E15`, not the Strom `A10`.
#[test]
fn e3002_zustimmung_is_e15_and_carries_no_ebd() {
    let e = pruefe_abmeldung_gas(&anfrage(44_007, Some("E03")), &lage());
    let a = e.as_antwort().expect("answer");
    assert_eq!(a.code, "E15");
    assert!(a.zustimmung);
    assert!(
        a.ebd.is_none(),
        "the Gas MIG does not name a Codeliste in STS DE 1131"
    );
}

/// `E_3002` — a missed Vorlauffrist is `E17`, the Gas Fristüberschreitung code.
#[test]
fn e3002_missed_frist_is_e17() {
    let mut l = lage();
    l.vorlauffrist_eingehalten = Bekannt::Nein;
    assert_eq!(
        code_of(&pruefe_abmeldung_gas(&anfrage(44_007, None), &l)),
        "E17"
    );
}

/// `E_3020` — a running contract is `Z12` „Ablehnung Vertragsbindung".
#[test]
fn e3020_running_contract_is_z12() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    assert_eq!(
        code_of(&pruefe_abmeldungsanfrage_gas(
            &anfrage(44_010, Some("E03")),
            &l
        )),
        "Z12"
    );
}

/// `E_3020` — `Z01` „Zustimmung mit Terminänderung" is admissible **only** with
/// Transaktionsgrund `E01`, which is what the code table's Bedingung says.
#[test]
fn e3020_terminaenderung_needs_transaktionsgrund_e01() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    l.vertragsende = Some(date!(2026 - 10 - 01));

    let umzug = pruefe_abmeldungsanfrage_gas(&anfrage(44_010, Some("E01")), &l);
    assert_eq!(code_of(&umzug), "Z01");
    assert_eq!(
        umzug.as_antwort().unwrap().termin,
        Some(date!(2026 - 10 - 01))
    );

    let wechsel = pruefe_abmeldungsanfrage_gas(&anfrage(44_010, Some("E03")), &l);
    assert_eq!(code_of(&wechsel), "Z12", "E03 has no Z01 Bedingung");
}

/// `E_3001` — a Kündigung to a date the contract already ended before is `Z29`.
#[test]
fn e3001_contract_already_gone_is_z29() {
    let mut l = lage();
    l.vertragsende = Some(date!(2026 - 07 - 01));
    assert_eq!(
        code_of(&pruefe_kuendigung_gas(&anfrage(44_016, None), &l)),
        "Z29"
    );
}

/// `E_3001` — a Vertragsbindung against a **fixed** date answers `Z12` and, per
/// the Codeliste's Anmerkung, must state the next possible Kündigungszeitpunkt.
#[test]
fn e3001_vertragsbindung_states_the_next_possible_date() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    // The date `Z12` must carry is the next *admissible* termination date, not
    // a Vertragsende: a bound contract has not been terminated at all.
    l.naechstmoeglicher_kuendigungstermin = Some(date!(2026 - 12 - 31));
    let e = pruefe_kuendigung_gas(&anfrage(44_016, None), &l);
    let a = e.as_antwort().expect("answer");
    assert_eq!(a.code, "Z12");
    assert_eq!(a.termin, Some(date!(2026 - 12 - 31)));
}

/// `E_3001` publishes no code for a Kündigungstermin in the past — the Strom
/// `A01`/`A10` has no Gas counterpart — and GeLi Gas 3.0 § 3.1 admits only „ein
/// beliebiges in der Zukunft liegendes … Kündigungsdatum". Falling through would
/// confirm it with `E15`.
#[test]
fn e3001_a_kuendigung_dated_in_the_past_escalates() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Nein;
    let mut a = anfrage(44_016, None);
    a.termin = Some(a.eingang.date().previous_day().expect("a day before"));
    assert!(pruefe_kuendigung_gas(&a, &l).ist_eskalation());

    // A Kündigung without any date has nothing to check at all.
    let mut a = anfrage(44_016, None);
    a.termin = None;
    assert!(pruefe_kuendigung_gas(&a, &l).ist_eskalation());
}

/// `E_3001` gates its two Vertragsbindungs-answers on the **date qualifier**,
/// exactly as `E_0614` Prüfschritt 60 does on the Strom side — and here the
/// gate is an AHB Bedingung, so getting it wrong fails validation rather than
/// merely stating the wrong thing:
///
/// - `Z12` „Ablehnung Vertragsbindung" is **`[43]` Wenn SG4 DTM+93 vorhanden**;
/// - `Z01` „Zustimmung mit Terminänderung" is **`[41]` Wenn SG4 DTM+471 vorhanden**.
///
/// So a Gas Kündigung „zum nächstmöglichen Termin" is *confirmed* at the date
/// the LFA determined. Refusing it with `Z12` would block every Gas switch of a
/// customer inside a Laufzeitvertrag.
#[test]
fn e3001_a_naechstmoeglich_kuendigung_is_confirmed_with_z01_not_refused() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    l.naechstmoeglicher_kuendigungstermin = Some(date!(2026 - 12 - 31));

    let mut a = anfrage(44_016, None);
    a.terminart = Terminart::Naechstmoeglich;

    let antwort = pruefe_kuendigung_gas(&a, &l);
    let antwort = antwort.as_antwort().expect("answer");
    assert_eq!(antwort.code, "Z01");
    assert!(
        antwort.zustimmung,
        "Z01 sits in the Zustimmungs-Cluster and rides 44017"
    );
    assert_eq!(antwort.termin, Some(date!(2026 - 12 - 31)));
}

/// Both codes have to name a date, so a deployment that cannot determine the
/// next admissible one escalates rather than sending an empty DTM segment.
#[test]
fn e3001_vertragsbindung_without_a_next_date_escalates() {
    let mut l = lage();
    l.vertragsbindung_am_folgetag = Bekannt::Ja;
    l.naechstmoeglicher_kuendigungstermin = None;
    for terminart in [Terminart::Fix, Terminart::Naechstmoeglich] {
        let mut a = anfrage(44_016, None);
        a.terminart = terminart;
        assert!(
            pruefe_kuendigung_gas(&a, &l).ist_eskalation(),
            "{terminart:?} without a next possible date must escalate"
        );
    }
}

// ── Cross-cutting invariants ──────────────────────────────────────────────────

/// Every landing of every tree must resolve to a code that tree publishes.
///
/// This is the guard the `# Panics` sections point at: the walks name codes as
/// string literals, and a typo would otherwise surface as a production panic.
#[test]
fn every_landing_resolves_to_a_published_code() {
    let gruende = [
        None,
        Some("E01"),
        Some("E03"),
        Some("Z33"),
        Some("ZC6"),
        Some("ZC7"),
    ];
    let arten = [
        Lokationsart::VerbrauchendeMalo,
        Lokationsart::ErzeugendeMalo,
        Lokationsart::Tranche,
        Lokationsart::RuhendeMalo,
    ];
    let werte = [Bekannt::Ja, Bekannt::Nein, Bekannt::Unbekannt];
    let vollmachten = [
        Vollmacht::NichtAngefordert,
        Vollmacht::AngefordertAusstehend,
        Vollmacht::Wirksam,
        Vollmacht::Unwirksam,
    ];

    /// `(EBD id, walk, the codes that EBD publishes)`.
    type Walk = (
        &'static str,
        fn(&LfAnfrage, &LfVertragslage) -> LfEntscheidung,
        &'static [mako_pruefung::AntwortCode],
    );

    let walks: [Walk; 6] = [
        ("E_0609", pruefe_abmeldung, E_0609_CODES),
        ("E_0624", pruefe_beendigung_zuordnung, E_0624_CODES),
        ("E_0614", pruefe_kuendigung, E_0614_CODES),
        ("E_3002", pruefe_abmeldung_gas, E_3002_CODES),
        ("E_3020", pruefe_abmeldungsanfrage_gas, E_3020_CODES),
        ("E_3001", pruefe_kuendigung_gas, E_3001_CODES),
    ];

    for (ebd, walk, published) in walks {
        for grund in gruende {
            for art in arten {
                for b in werte {
                    for vm in vollmachten {
                        let mut a = anfrage(55_007, grund);
                        a.lokationsart = Some(art);
                        let mut l = lage();
                        l.vertragsbindung_am_folgetag = b;
                        l.kunde_identisch = b;
                        l.kunde_nicht_ausgezogen = b;
                        l.keine_stilllegung = b;
                        l.zuordnung_am_folgetag = b;
                        l.zrt_wechsel_mit_ermaechtigung = b;
                        l.zuordnungsermaechtigung_deaktiviert = b;
                        l.vorlauffrist_eingehalten = b;
                        l.in_ersatzversorgung_am_folgetag = b;
                        l.vollmacht = vm;
                        if let Some(answer) = walk(&a, &l).as_antwort() {
                            assert!(
                                published.iter().any(|c| c.code == answer.code),
                                "{ebd} produced {}, which it does not publish",
                                answer.code
                            );
                        }
                    }
                }
            }
        }
    }
}

/// A code that requires an Erläuterung never leaves without one.
#[test]
fn codes_that_require_a_bemerkung_always_carry_one() {
    let mut l = lage();
    l.keine_stilllegung = Bekannt::Ja;
    let e = pruefe_abmeldung(&anfrage(55_007, Some("Z33")), &l);
    let a = e.as_antwort().expect("answer");
    assert!(
        lookup("E_0609", &a.code)
            .expect("published")
            .braucht_bemerkung
    );
    assert!(a.bemerkung.is_some());
}
