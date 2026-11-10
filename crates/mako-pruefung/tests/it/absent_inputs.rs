//! No production walk ends in acceptance on an input it does not have.
//!
//! An Entscheidungsbaum is a sequence of questions, and a Prüfschritt whose
//! answer the caller's records do not hold has no outcome the tree publishes:
//! the walk escalates to an operator or refuses, naming the step. A walk that
//! reads `None` as the positive answer — or a step that answers all-clear
//! whatever its input — puts a Zustimmung on the market that nothing decided.
//!
//! Each case below starts from an input its walk accepts. The harness
//! serialises it, takes every value away once — `null` for an `Option`,
//! `UNBEKANNT` for a tri-state such as [`mako_pruefung::Bekannt`] or the
//! internally tagged [`mako_pruefung::msb::Vertragslage`] — keeps the variants
//! that still deserialise (those are the inputs a caller can leave absent),
//! walks each, and fails on every one that is still accepted. A value that is
//! `None` in the accepting input is a fact (no confirmed Zuordnungsende, no
//! Fehler), not an absent input, and is never taken away.
//!
//! An input whose absence the tree itself answers is listed in the case's
//! `exempt` table with the document that says so. The table is a ratchet in
//! both directions: an unlisted acceptance fails, and so does a listed path
//! that no longer accepts.
//!
//! # The production walks
//!
//! Every walk a service or makod calls: `processd` (NB, LF, MSB, ESA,
//! Neuanlage modules), `invoicd` / `invoic-checker` (the invoice walk and the
//! Stornorechnung), `mako-mabis` (the Listenabgleich) and makod's WiM door
//! (the Weiterverpflichtung). Walks with no caller outside this crate — the
//! MaBiS ZP, Zeitreihe, Profil and Ausfallarbeit trees, the WiM Ersteinbau and
//! Technik trees, the NZR-EMob Modellwechsel trees and `E_0265`
//! `pruefe_nicht_zahlungsavis` — are not production walks yet and join here
//! with their first caller.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// One way of taking a value away: where, how, and the input it leaves.
struct Absence {
    path: String,
    label: String,
    mutated: Value,
}

/// Every way of taking one value of `v` away.
fn absences(v: &Value) -> Vec<Absence> {
    let mut out = Vec::new();
    walk_paths(v, String::new(), &mut |path, node| {
        let mut push = |label: String, replacement: Value| {
            let mut m = v.clone();
            *m.pointer_mut(&path).expect("the path exists") = replacement;
            out.push(Absence {
                path: path.clone(),
                label,
                mutated: m,
            });
        };
        if !node.is_null() {
            push(format!("{path}=null"), Value::Null);
        }
        if node.is_string() {
            push(format!("{path}=UNBEKANNT"), Value::from("UNBEKANNT"));
        }
        // An internally tagged enum: its tag alone, set to the unknown variant.
        if let Some(obj) = node.as_object() {
            for (k, tag) in obj {
                if tag.is_string() {
                    push(
                        format!("{path}={{{k}:UNBEKANNT}}"),
                        serde_json::json!({ k: "UNBEKANNT" }),
                    );
                }
            }
        }
    });
    out
}

/// The known values a yes/no answer at `path` can take instead: the other
/// boolean, or the other side of a `JA`/`NEIN` tri-state. Empty when the
/// value is not such an answer.
fn flips(base: &Value, path: &str) -> Vec<Value> {
    let node = base.pointer(path).expect("the path exists");
    let others: Vec<Value> = match node {
        Value::Bool(b) => vec![Value::Bool(!b)],
        Value::String(s) if s == "JA" => vec![Value::from("NEIN")],
        Value::String(s) if s == "NEIN" => vec![Value::from("JA")],
        _ => Vec::new(),
    };
    others
        .into_iter()
        .map(|o| {
            let mut m = base.clone();
            *m.pointer_mut(path).expect("the path exists") = o;
            m
        })
        .collect()
}

fn walk_paths(v: &Value, path: String, f: &mut impl FnMut(String, &Value)) {
    if !path.is_empty() {
        f(path.clone(), v);
    }
    match v {
        Value::Object(o) => {
            for (k, child) in o {
                walk_paths(
                    child,
                    format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                    f,
                );
            }
        }
        Value::Array(a) => {
            for (i, child) in a.iter().enumerate() {
                walk_paths(child, format!("{path}/{i}"), f);
            }
        }
        _ => {}
    }
}

/// One production walk from one accepting input.
struct Case<T> {
    name: &'static str,
    input: T,
    accepts: fn(&T) -> bool,
    /// `(label, reason)` — an absence the tree itself answers with acceptance.
    exempt: &'static [(&'static str, &'static str)],
}

/// Walk every absence of `case`; return what failed.
fn check<T: Serialize + DeserializeOwned>(case: &Case<T>) -> Vec<String> {
    let mut failures = Vec::new();
    if !(case.accepts)(&case.input) {
        failures.push(format!("{}: the baseline is not accepted", case.name));
        return failures;
    }
    let base = serde_json::to_value(&case.input).expect("serialises");
    let mut used = std::collections::BTreeSet::new();
    let parses = |v: Value| serde_json::from_value::<T>(v).ok();
    for Absence {
        path,
        label,
        mutated,
    } in absences(&base)
    {
        // `UNBEKANNT` on a free-text field is a value, not an absence.
        if label.contains("UNBEKANNT") {
            let free: Value = serde_json::from_str(
                &mutated
                    .to_string()
                    .replace("UNBEKANNT", "kein Variantenname"),
            )
            .expect("still JSON");
            if parses(free).is_some() {
                continue;
            }
        }
        let Some(input) = parses(mutated) else {
            continue;
        };
        // A mutation the type normalises back to the baseline took nothing away.
        if serde_json::to_value(&input).expect("serialises") == base {
            continue;
        }
        // A yes/no answer this walk does not read on this path is no input of
        // it: neither of its known values changes the outcome.
        let flipped = flips(&base, &path);
        if !flipped.is_empty()
            && flipped
                .into_iter()
                .filter_map(parses)
                .all(|f| (case.accepts)(&f))
        {
            continue;
        }
        if (case.accepts)(&input) {
            if case.exempt.iter().any(|(l, _)| *l == label) {
                used.insert(label);
            } else {
                failures.push(format!("{}: accepted with {label}", case.name));
            }
        }
    }
    for (label, _) in case.exempt {
        if !used.contains(*label) {
            failures.push(format!(
                "{}: {label} is exempt but no longer accepted — delete the row",
                case.name
            ));
        }
    }
    failures
}

fn run_all(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} absent input(s) end in acceptance:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// The harness itself: a walk that reads an absent Prüfschritt answer as the
/// positive one is caught, and one that escalates is not.
#[test]
fn a_walk_reading_none_as_yes_is_caught() {
    #[derive(Serialize, serde::Deserialize)]
    struct Fakten {
        frist_eingehalten: Option<bool>,
    }
    let lax = Case {
        name: "lax",
        input: Fakten {
            frist_eingehalten: Some(true),
        },
        accepts: |f| f.frist_eingehalten.unwrap_or(true),
        exempt: &[],
    };
    assert_eq!(check(&lax), ["lax: accepted with /frist_eingehalten=null"]);
    let strict = Case {
        name: "strict",
        input: Fakten {
            frist_eingehalten: Some(true),
        },
        accepts: |f| f.frist_eingehalten == Some(true),
        exempt: &[],
    };
    assert!(check(&strict).is_empty());
}

// ── LF ────────────────────────────────────────────────────────────────────────

#[cfg(feature = "role-lf")]
mod lf {
    use super::{Case, check, run_all};
    use mako_pruefung::lf::{EogZustaendigkeit, Lokationsart, ZuordnungsLage};
    use mako_pruefung::{
        Bekannt, LfAnfrage, LfEntscheidung, LfVertragslage, Terminart, Vollmacht, ZuordnungsFall,
    };
    use serde::{Deserialize, Serialize};
    use time::macros::{date, datetime};

    fn anfrage(pid: u32, grund: &str) -> LfAnfrage {
        LfAnfrage {
            pid,
            process_id: uuid::Uuid::nil(),
            malo_id: "51238696012".to_owned(),
            // Echoed in the answer's `RFF+TN`, asked by no Prüfschritt.
            vorgangsnummer: None,
            absender_mp_id: "9900000000002".to_owned(),
            empfaenger_mp_id: "9900357000004".to_owned(),
            lokationsart: Some(Lokationsart::VerbrauchendeMalo),
            transaktionsgrund: Some(grund.to_owned()),
            termin: Some(date!(2026 - 08 - 01)),
            terminart: Terminart::Fix,
            uet_lieferanmeldung: Some(date!(2026 - 07 - 01)),
            eingang: datetime!(2026-07-01 08:00 UTC),
        }
    }

    fn lage() -> LfVertragslage {
        LfVertragslage {
            beliefert: true,
            vertrag_vorhanden: Bekannt::Ja,
            zuordnung_am_folgetag: Bekannt::Ja,
            bestaetigtes_zuordnungsende: None,
            bestaetigter_zuordnungsbeginn: None,
            vertragsende: None,
            naechstmoeglicher_kuendigungstermin: None,
            vertragsbindung_am_folgetag: Bekannt::Nein,
            kunde_identisch: Bekannt::Ja,
            kunde_nicht_ausgezogen: Bekannt::Nein,
            ist_grundversorger: false,
            in_ersatzversorgung_am_folgetag: Bekannt::Nein,
            keine_stilllegung: Bekannt::Nein,
            zrt_wechsel_mit_ermaechtigung: Bekannt::Nein,
            zuordnungsermaechtigung_deaktiviert: Bekannt::Ja,
            vorlauffrist_eingehalten: Bekannt::Ja,
            vollmacht: Vollmacht::Wirksam,
            eigene_abmeldung_offen: false,
        }
    }

    /// A contract terminable to the Kündigungstermin.
    fn kuendbar() -> LfVertragslage {
        LfVertragslage {
            naechstmoeglicher_kuendigungstermin: Some(date!(2026 - 08 - 01)),
            ..lage()
        }
    }

    // Inputs a walk does not read. Each is a field of the parsed message that
    // some other tree asks about; absent here, it decides nothing.
    const UET: (&str, &str) = (
        "/anfrage/uet_lieferanmeldung=null",
        "not asked: the ÜT der Lieferanmeldung is the anchor of E_0624 Prüfschritt 5 \
         alone (EBD 4.3 Kap. 6.6.3, S. 81)",
    );
    const LOKATIONSART_GAS: (&str, &str) = (
        "/anfrage/lokationsart=null",
        "not asked: the Gas Codelisten (EBD 4.3 Kap. 13) do not branch on a \
         Transaktionsgrundergänzung",
    );
    const LOKATIONSART_EOG: (&str, &str) = (
        "/anfrage/lokationsart=null",
        "not asked: E_0615 Prüfschritte 20–90 (EBD 4.3 Kap. 6.7.1, S. 89) ask nothing \
         about the Lokationsart",
    );
    const GRUND_EOG: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_0615 Prüfschritte 20–90 (EBD 4.3 Kap. 6.7.1, S. 89) do not branch \
         on the Transaktionsgrund",
    );
    const TERMIN_ZUORDNUNG: (&str, &str) = (
        "/anfrage/termin=null",
        "not asked: E_0603–E_0606 publish the one Prüfschritt 10 „Ist ein zuvor nicht \
         spezifizierter Fehler aufgetreten?\" (EBD 4.3 Kap. 6.51.1, S. 420)",
    );
    const LOKATIONSART_ZUORDNUNG: (&str, &str) = (
        "/anfrage/lokationsart=null",
        "not asked: the Fall (ZW8/ZW9/ZX0/ZX1) selects the tree, and its one Prüfschritt \
         10 asks nothing else (EBD 4.3 Kap. 6.51.1, S. 420)",
    );
    const GRUND_ZUORDNUNG: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_0603–E_0606 Prüfschritt 10 is the only step (EBD 4.3 Kap. 6.51.1, \
         S. 420)",
    );
    const TERMIN_EOG_GAS: (&str, &str) = (
        "/anfrage/termin=null",
        "not asked: E_3008 publishes Codelisten and no Prüfschritte (EBD 4.3 Kap. 13.7.1, \
         S. 728 f.); Z08 „Transaktion schon stattgefunden\" is Nutzung O and not walked",
    );
    const GRUND_EOG_GAS: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_3008 publishes Codelisten and no Prüfschritte (EBD 4.3 Kap. 13.7.1, \
         S. 728 f.)",
    );
    const FRIST_EOG_GAS: (&str, &str) = (
        "/zustaendigkeit/frist_eingehalten=null",
        "E_3008 publishes no Prüfschritt asking the Frist (EBD 4.3 Kap. 13.7.1, S. 728 f.): \
         E17 „Ablehnung wg. Fristüberschreitung\" is Nutzung O, a refusal the E/G may raise \
         on a finding it holds",
    );
    const FRIST_ABMELDUNGSANFRAGE_GAS: (&str, &str) = (
        "/lage/vorlauffrist_eingehalten=UNBEKANNT",
        "E_3020 publishes Codelisten and no Prüfschritte (EBD 4.3 Kap. 13.6.3, S. 723): \
         E17 „Ablehnung wg. Fristüberschreitung\" is Nutzung O, and the Frist it refuses \
         runs on the LFN's Anmeldung, which the Abmeldungsanfrage does not carry",
    );
    const GRUND_KUENDIGUNG: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_0614 Prüfschritte 10–630 (EBD 4.3 Kap. 6.2.1, S. 46–51) do not \
         branch on the Transaktionsgrund",
    );
    const GRUND_KUENDIGUNG_GAS: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_3001 publishes Codelisten and no Prüfschritte (EBD 4.3 Kap. 13.3.1, \
         S. 714 f.), and none of its codes turns on the Transaktionsgrund",
    );
    const GRUND_TRANCHE: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_0624 Prüfschritte 200–220 ask the Zuordnung, the bestätigte \
         Zuordnungsende and the Vertragsbindung (EBD 4.3 Kap. 6.6.3, S. 83)",
    );
    const GRUND_ABMELDUNGSANFRAGE_GAS: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "read only where the Vertragsbindung refuses: G_0010 admits Z01 instead of Z12 \
         for E01 alone (EBD 4.3 Kap. 13.6.3, S. 723 f.); the E15 path does not branch on it",
    );

    #[derive(Serialize, Deserialize)]
    struct Zwei {
        anfrage: LfAnfrage,
        lage: LfVertragslage,
    }

    fn zwei(
        name: &'static str,
        anfrage: LfAnfrage,
        lage: LfVertragslage,
        accepts: fn(&Zwei) -> bool,
        exempt: &'static [(&'static str, &'static str)],
    ) -> Vec<String> {
        check(&Case {
            name,
            input: Zwei { anfrage, lage },
            accepts,
            exempt,
        })
    }

    fn ja(e: &LfEntscheidung) -> bool {
        e.ist_zustimmung()
    }

    #[test]
    fn no_lf_walk_accepts_an_absent_input() {
        let mut f = Vec::new();

        // E_0609 — Abmeldung (55007), the Zuordnungsermächtigung branch.
        f.extend(zwei(
            "E_0609 pruefe_abmeldung",
            anfrage(55_007, mako_pruefung::BKV_DEAKTIVIERUNG),
            lage(),
            |z| ja(&mako_pruefung::pruefe_abmeldung(&z.anfrage, &z.lage)),
            &[UET],
        ));
        // E_0609 — the Stilllegung branch (Prüfschritte 50/60).
        f.extend(zwei(
            "E_0609 pruefe_abmeldung (Z33)",
            anfrage(55_007, mako_pruefung::STILLLEGUNG),
            lage(),
            |z| ja(&mako_pruefung::pruefe_abmeldung(&z.anfrage, &z.lage)),
            &[UET],
        ));
        // E_0624 — the Umzug branch (Prüfschritte 40–60) and a Tranche (200–220).
        f.extend(zwei(
            "E_0624 pruefe_beendigung_zuordnung (E01)",
            anfrage(55_010, mako_pruefung::EIN_AUSZUG),
            LfVertragslage {
                kunde_identisch: Bekannt::Nein,
                ..lage()
            },
            |z| {
                ja(&mako_pruefung::pruefe_beendigung_zuordnung(
                    &z.anfrage, &z.lage,
                ))
            },
            &[],
        ));
        f.extend(zwei(
            "E_0624 pruefe_beendigung_zuordnung (Tranche)",
            LfAnfrage {
                lokationsart: Some(Lokationsart::Tranche),
                ..anfrage(55_010, mako_pruefung::WECHSEL)
            },
            LfVertragslage {
                zuordnung_am_folgetag: Bekannt::Nein,
                ..lage()
            },
            |z| {
                ja(&mako_pruefung::pruefe_beendigung_zuordnung(
                    &z.anfrage, &z.lage,
                ))
            },
            &[GRUND_TRANCHE],
        ));
        // E_3002 — Abmeldung Gas.
        f.extend(zwei(
            "Gas pruefe_abmeldung_gas",
            anfrage(44_004, mako_pruefung::EIN_AUSZUG),
            lage(),
            |z| ja(&mako_pruefung::pruefe_abmeldung_gas(&z.anfrage, &z.lage)),
            &[UET, LOKATIONSART_GAS],
        ));
        // E_0624 — Beendigung der Zuordnung (55010).
        f.extend(zwei(
            "E_0624 pruefe_beendigung_zuordnung",
            anfrage(55_010, mako_pruefung::WECHSEL),
            lage(),
            |z| {
                ja(&mako_pruefung::pruefe_beendigung_zuordnung(
                    &z.anfrage, &z.lage,
                ))
            },
            &[],
        ));
        // E_3020 — Abmeldungsanfrage Gas.
        f.extend(zwei(
            "Gas pruefe_abmeldungsanfrage_gas",
            anfrage(44_010, mako_pruefung::WECHSEL),
            lage(),
            |z| {
                ja(&mako_pruefung::pruefe_abmeldungsanfrage_gas(
                    &z.anfrage, &z.lage,
                ))
            },
            &[
                UET,
                LOKATIONSART_GAS,
                FRIST_ABMELDUNGSANFRAGE_GAS,
                GRUND_ABMELDUNGSANFRAGE_GAS,
            ],
        ));
        // E_0614 — Kündigung.
        f.extend(zwei(
            "E_0614 pruefe_kuendigung",
            anfrage(55_016, mako_pruefung::WECHSEL),
            kuendbar(),
            |z| ja(&mako_pruefung::pruefe_kuendigung(&z.anfrage, &z.lage)),
            &[UET, GRUND_KUENDIGUNG],
        ));
        // E_3001 — Kündigung Gas.
        f.extend(zwei(
            "Gas pruefe_kuendigung_gas",
            anfrage(44_016, mako_pruefung::WECHSEL),
            kuendbar(),
            |z| ja(&mako_pruefung::pruefe_kuendigung_gas(&z.anfrage, &z.lage)),
            &[UET, LOKATIONSART_GAS, GRUND_KUENDIGUNG_GAS],
        ));

        // E_0615 / E_3008 — Anmeldung EoG.
        #[derive(Serialize, Deserialize)]
        struct Eog {
            anfrage: LfAnfrage,
            lage: LfVertragslage,
            zustaendigkeit: EogZustaendigkeit,
        }
        let zustaendigkeit = EogZustaendigkeit {
            zustaendig: Some(true),
            frist_eingehalten: Some(true),
            gesetzliche_pflicht: Some(true),
        };
        f.extend(check(&Case {
            name: "E_0615 pruefe_anmeldung_eog",
            input: Eog {
                anfrage: anfrage(55_013, "ZC6"),
                lage: lage(),
                zustaendigkeit,
            },
            accepts: |e| {
                ja(&mako_pruefung::pruefe_anmeldung_eog(
                    &e.anfrage,
                    &e.lage,
                    &e.zustaendigkeit,
                ))
            },
            exempt: &[UET, LOKATIONSART_EOG, GRUND_EOG],
        }));
        f.extend(check(&Case {
            name: "Gas pruefe_anmeldung_eog_gas",
            input: Eog {
                anfrage: anfrage(44_013, mako_pruefung::EIN_AUSZUG),
                lage: lage(),
                zustaendigkeit,
            },
            accepts: |e| {
                ja(&mako_pruefung::pruefe_anmeldung_eog_gas(
                    &e.anfrage,
                    &e.lage,
                    &e.zustaendigkeit,
                ))
            },
            exempt: &[
                UET,
                LOKATIONSART_GAS,
                TERMIN_EOG_GAS,
                GRUND_EOG_GAS,
                FRIST_EOG_GAS,
            ],
        }));

        // E_0603–E_0606 — Ankündigung Zuordnung LF (55607).
        #[derive(Serialize, Deserialize)]
        struct Zuordnung {
            anfrage: LfAnfrage,
            fall: Option<ZuordnungsFall>,
            lage: ZuordnungsLage,
        }
        f.extend(check(&Case {
            name: "E_0604 pruefe_zuordnung",
            input: Zuordnung {
                anfrage: anfrage(55_607, "ZW9"),
                fall: Some(ZuordnungsFall::EegMitDvPflicht),
                lage: ZuordnungsLage {
                    bilanzkreis: Some("11XBK-STD------Y".to_owned()),
                    fehler: None,
                },
            },
            accepts: |z| {
                ja(&mako_pruefung::pruefe_zuordnung(
                    &z.anfrage, z.fall, &z.lage,
                ))
            },
            exempt: &[
                UET,
                LOKATIONSART_ZUORDNUNG,
                TERMIN_ZUORDNUNG,
                GRUND_ZUORDNUNG,
            ],
        }));

        run_all(f);
    }
}

// ── MSB and ESA ───────────────────────────────────────────────────────────────

#[cfg(feature = "role-msb")]
mod msb {
    use super::{Case, check, run_all};
    use mako_pruefung::HolidayCalendar;
    use mako_pruefung::esa::wertebestellung::{
        Bestellart, EsaAnfrage, EsaBeendigung, EsaBestellung, EsaStornierung,
    };
    use mako_pruefung::msb::types::{RahmenvertragBefund, Sparte};
    use mako_pruefung::msb::{
        Abmeldegrund, AbmeldungMsb, AnmeldungMsb, Einrichtungsart, KuendigungMsb,
        Kuendigungstermin, MsbEntscheidung, Vertragslage, WeiterverpflichtungAuftrag,
    };
    use serde::{Deserialize, Serialize};
    use time::macros::date;

    fn ja(e: &MsbEntscheidung) -> bool {
        matches!(e, MsbEntscheidung::Accept(_))
    }

    /// The MSB holds the ESA's Zusicherung, not the consent itself.
    const EINWILLIGUNG: &str = "an absent consent record is the designed state: WiM Teil 2 \
        UC 4.1.1 (S. 70) names as Vorbedingung „Die Zusicherung des ESA zur Einhaltung der \
        rechtlichen Vorgaben zum Datenschutz liegt beim MSB vor\", and only a record stating \
        the consent invalid refuses (E_0252 Prüfschritte 4/5, E_0256 Prüfschritt 8)";

    /// The process record holds every delivery it sent.
    const JUENGSTE_LIEFERUNG: (&str, &str) = (
        "/juengste_lieferung=null",
        "a fact, not an absence: the Wertebestellung process folds every delivery into it, \
         so none means no values went out, and E_0254 Prüfschritt 4 „Liegt das gewünschte \
         Beendigungsdatum vor dem Zeitpunkt, für den die jüngsten Daten geliefert wurden?\" \
         is answered nein (EBD 4.3 Kap. 8.26.1, S. 622)",
    );

    #[test]
    fn no_msb_or_esa_walk_accepts_an_absent_input() {
        let mut f = Vec::new();

        #[derive(Serialize, Deserialize)]
        struct Anmeldung {
            anfrage: AnmeldungMsb,
            eingang: time::Date,
        }
        f.extend(check(&Case {
            name: "E_0201 pruefe_anmeldung",
            input: Anmeldung {
                anfrage: AnmeldungMsb {
                    melo_id: "DE00056266802AO6G56M11SN51G21M24S".to_owned(),
                    msbn_mp_id: "9900357000004".to_owned(),
                    gewuenschter_zuordnungsbeginn: date!(2026 - 06 - 01),
                    sparte: Sparte::Strom,
                    einrichtungsart: Einrichtungsart::BestehenderMessstellenbetrieb,
                    versicherung_liegt_vor: true,
                    melo_bekannt: Some(true),
                    msb_rahmenvertrag: Some(RahmenvertragBefund::Abgeschlossen),
                },
                eingang: mako_fristen::sub_werktage(
                    date!(2026 - 06 - 01),
                    15,
                    HolidayCalendar::BdewMaKo,
                ),
            },
            accepts: |a| {
                ja(&mako_pruefung::msb::pruefe_anmeldung(
                    &a.anfrage,
                    a.eingang,
                    HolidayCalendar::BdewMaKo,
                ))
            },
            exempt: &[],
        }));

        #[derive(Serialize, Deserialize)]
        struct Abmeldung {
            anfrage: AbmeldungMsb,
            eingang: time::Date,
        }
        f.extend(check(&Case {
            name: "E_0202 pruefe_abmeldung",
            input: Abmeldung {
                anfrage: AbmeldungMsb {
                    melo_id: "DE00056266802AO6G56M11SN51G21M24S".to_owned(),
                    msba_mp_id: "9900357000004".to_owned(),
                    gewuenschtes_zuordnungsende: date!(2026 - 06 - 01),
                    sparte: Sparte::Strom,
                    grund: Abmeldegrund::VertragsEnde,
                    zuordnung_besteht: Some(true),
                },
                eingang: date!(2026 - 04 - 01),
            },
            accepts: |a| {
                ja(&mako_pruefung::msb::pruefe_abmeldung(
                    &a.anfrage,
                    a.eingang,
                    HolidayCalendar::BdewMaKo,
                ))
            },
            exempt: &[],
        }));

        f.extend(check(&Case {
            name: "E_0200 pruefe_kuendigung",
            input: KuendigungMsb {
                melo_id: "DE00056266802AO6G56M11SN51G21M24S".to_owned(),
                msbn_mp_id: "9900357000004".to_owned(),
                sparte: Sparte::Strom,
                kuendigungstermin: Kuendigungstermin::Fix(date!(2026 - 09 - 01)),
                vertragslage: Vertragslage::Laufend {
                    naechstmoeglich: date!(2026 - 08 - 01),
                },
            },
            accepts: |k| ja(&mako_pruefung::msb::pruefe_kuendigung(k)),
            exempt: &[],
        }));

        f.extend(check(&Case {
            name: "pruefe_weiterverpflichtung",
            input: WeiterverpflichtungAuftrag {
                melo_id: "DE00056266802AO6G56M11SN51G21M24S".to_owned(),
                bestaetigtes_zuordnungsende: date!(2026 - 06 - 01),
                verschobenes_zuordnungsende: date!(2026 - 06 - 15),
                sparte: Sparte::Strom,
                grund: Abmeldegrund::VertragsEnde,
                bereits_ausgeschoepft: false,
            },
            accepts: |w| ja(&mako_pruefung::msb::pruefe_weiterverpflichtung(w)),
            exempt: &[],
        }));

        // ESA Wertebestellung — E_0252, E_0256, E_0257, E_0254.
        let anfrage = EsaAnfrage {
            pflichtprodukt: false,
            messprodukt_angeboten: Some(true),
            vertrag_vorhanden: true,
            einwilligung_vorhanden: Some(true),
            einwilligung_plausibel: Some(true),
            geraetetechnik_geeignet: true,
            gebuendelte_ebene: true,
            msb_aller_messlokationen: Some(true),
        };
        f.extend(check(&Case {
            name: "E_0252 pruefe_anfrage",
            input: anfrage,
            accepts: |a| ja(&mako_pruefung::esa::wertebestellung::pruefe_anfrage(a)),
            exempt: &[
                ("/einwilligung_vorhanden=null", EINWILLIGUNG),
                ("/einwilligung_plausibel=null", EINWILLIGUNG),
            ],
        }));
        f.extend(check(&Case {
            name: "E_0256 pruefe_bestellung",
            input: EsaBestellung {
                bindungsfrist: time::macros::datetime!(2026-07-01 00:00 UTC),
                eingegangen_am: time::macros::datetime!(2026-06-20 10:00 UTC),
                akzeptiert_nach_bindungsfrist: false,
                art: Bestellart::Abo,
                messprodukt_lieferbar: true,
                vertrag_gueltig: true,
                zugeordnet: true,
                einwilligung_gueltig: Some(true),
                geraetetechnik_geeignet: true,
                gebuendelte_ebene: true,
                msb_aller_messlokationen: Some(true),
            },
            accepts: |b| ja(&mako_pruefung::esa::wertebestellung::pruefe_bestellung(b)),
            exempt: &[("/einwilligung_gueltig=null", EINWILLIGUNG)],
        }));
        f.extend(check(&Case {
            name: "E_0257 pruefe_stornierung",
            input: EsaStornierung {
                bestellung_bestaetigt: true,
                art: Bestellart::Abo,
                uebermittlung_begonnen: false,
            },
            accepts: |s| ja(&mako_pruefung::esa::wertebestellung::pruefe_stornierung(s)),
            exempt: &[],
        }));
        f.extend(check(&Case {
            name: "E_0254 pruefe_beendigung",
            input: EsaBeendigung {
                art: Bestellart::Abo,
                beendigung_zum: date!(2026 - 09 - 01),
                abo_beginn: date!(2026 - 01 - 01),
                bereits_beendet_zum: None,
                juengste_lieferung: Some(date!(2026 - 08 - 01)),
            },
            accepts: |b| ja(&mako_pruefung::esa::wertebestellung::pruefe_beendigung(b)),
            exempt: &[JUENGSTE_LIEFERUNG],
        }));

        run_all(f);
    }
}

// ── NB ────────────────────────────────────────────────────────────────────────

#[cfg(feature = "role-nb")]
mod nb {
    use super::{Case, check, run_all};
    use mako_markt::domain::Sparte;
    use mako_markt::repository::{
        LfZuordnung, LieferStatus, VersorgungsStatusRecord, ZuordnungsStatus,
    };
    use mako_pruefung::nb::neuanlage::{
        Identifikation, NeuanlageAnfrage, NeuanlageBefund, NeuanlageEntscheidung,
    };
    use mako_pruefung::nb::types::{
        ErzeugungsAnmeldung, Geschaeftsvorfall, Marktlokationsart, RuhendeAnmeldung,
        Veraeusserungsform,
    };
    use mako_pruefung::{
        Abmeldeanfrage, AbmeldungAnfrage, AnmeldungAnfrage, LfaAntwort, MaloGridRecord, Messtyp,
        NbEntscheidung, NetzCheckConfig,
    };
    use serde::{Deserialize, Serialize};
    use time::OffsetDateTime;
    use time::macros::{date, datetime};

    const LFN: &str = "9900357000004";
    const NB: &str = "9900000000002";

    fn ja(e: &NbEntscheidung) -> bool {
        matches!(e, NbEntscheidung::Accept(_))
    }

    /// The Anmeldung's own claim, held against the grid record when made.
    const BILANZIERUNGSGEBIET: (&str, &str) = (
        "/anfrage/bilanzierungsgebiet=null",
        "the LF's own statement (SG10 CCI+Z20), not the NB's record: when present it must \
         match the grid record, and absent there is nothing to contradict",
    );
    const GRUND_E0622: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_0622 Prüfschritte 10–70 (EBD 4.3 Kap. 6.6.1, S. 72–75) branch on the \
         Transaktionsgrundergänzung, which the Marktlokationsart carries, not on the \
         Transaktionsgrund",
    );
    const GRUND_G0011: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "beyond the Wechsel lead every Transaktionsgrund's Frist holds: a Wechsel needs \
         the ten Werktage the date already keeps, an Ein-/Auszug only a date not in the past",
    );
    const GRUND_E0623: (&str, &str) = (
        "/anfrage/transaktionsgrund=null",
        "not asked: E_0623 Prüfschritte 10–60 (EBD 4.3 Kap. 6.6.4, S. 85)",
    );
    const BILANZIERUNGSGEBIET_E0623: (&str, &str) = (
        "/anfrage/bilanzierungsgebiet=null",
        "not asked: E_0623 Prüfschritte 10–60 (EBD 4.3 Kap. 6.6.4, S. 85)",
    );
    const LFA_SCHWEIGT: (&str, &str) = (
        "/anfrage/abmeldeanfrage/antwort=null",
        "the tree's own answer: E_0623 Prüfschritt 30 „Hat der LFA fristgerecht \
         geantwortet?\" nein → 60 → A51 (EBD 4.3 Kap. 6.6.4, S. 85)",
    );
    const LFA_ZUORDNUNGSENDE: (&str, &str) = (
        "/anfrage/abmeldeanfrage/antwort/zuordnungsende=null",
        "not asked: E_0623 Prüfschritt 40 asks whether the LFA objected, not to which date \
         (EBD 4.3 Kap. 6.6.4, S. 85)",
    );

    fn anmeldung(pid: u32, process_date: time::Date) -> AnmeldungAnfrage {
        AnmeldungAnfrage {
            pid,
            process_id: uuid::Uuid::nil(),
            malo_id: "51238696012".to_owned(),
            new_supplier_gln: LFN.to_owned(),
            grid_operator_gln: NB.to_owned(),
            bilanzierungsgebiet: Some("11YB-TENNET----U".to_owned()),
            process_date,
            sparte: Sparte::Strom,
            messtyp: Messtyp::Slp,
            transaktionsgrund: Some("E03".to_owned()),
            marktlokationsart: Marktlokationsart::Verbrauchend,
            erzeugung: None,
            abmeldeanfrage: Abmeldeanfrage::NichtErforderlich,
            ruhend: mako_pruefung::RuhendeAnmeldung::default(),
        }
    }

    fn versorgung(status: LieferStatus, lf: Option<&str>) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: "51238696012".parse().expect("valid MaLo"),
            lieferstatus: status,
            zuordnungen: lf
                .map(|lf| vec![LfZuordnung::ganz(lf, ZuordnungsStatus::Aktiv)])
                .unwrap_or_default(),
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: NB.to_owned(),
            eog_seit: None,
            last_process_id: None,
            updated_at: datetime!(2026-01-01 00:00 UTC),
            tenant: NB.to_owned(),
            version: 1,
        }
    }

    fn grid() -> MaloGridRecord {
        MaloGridRecord {
            malo_id: "51238696012".to_owned(),
            nb_mp_id: NB.to_owned(),
            bilanzierungsgebiet: Some("11YB-TENNET----U".to_owned()),
            netzgebiet: None,
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Anmeldung {
        anfrage: AnmeldungAnfrage,
        versorgung: Option<VersorgungsStatusRecord>,
        grid: Option<MaloGridRecord>,
        #[serde(with = "time::serde::rfc3339")]
        now: OffsetDateTime,
    }

    fn evaluate(a: &Anmeldung) -> bool {
        ja(&mako_pruefung::evaluate(
            &a.anfrage,
            a.versorgung.as_ref(),
            a.grid.as_ref(),
            true,
            a.now,
            &NetzCheckConfig::default(),
        ))
    }

    #[test]
    fn no_nb_walk_accepts_an_absent_input() {
        let mut f = Vec::new();

        // E_0622 / E_0623 — Anmeldung verbrauchend.
        f.extend(check(&Case {
            name: "E_0622 evaluate (verbrauchend)",
            input: Anmeldung {
                anfrage: anmeldung(55_001, date!(2026 - 07 - 10)),
                versorgung: Some(versorgung(LieferStatus::Grundversorgung, None)),
                grid: Some(grid()),
                now: datetime!(2026-07-08 10:00 UTC),
            },
            accepts: evaluate,
            exempt: &[BILANZIERUNGSGEBIET, GRUND_E0622],
        }));
        // E_0622 — Anmeldung einer ruhenden Marktlokation (Prüfschritte 18–28).
        f.extend(check(&Case {
            name: "E_0622 evaluate (ruhend)",
            input: Anmeldung {
                anfrage: AnmeldungAnfrage {
                    marktlokationsart: Marktlokationsart::Ruhend,
                    ruhend: RuhendeAnmeldung {
                        ruhende_ist_verbrauchend: Some(true),
                        neubildung_kundenanlage: Some(false),
                        virtueller_summenzaehler: Some(true),
                        ruhende_ims: Some(true),
                        z16_ist_kundenanlage: Some(true),
                        gleiche_netzlokationen: Some(true),
                    },
                    ..anmeldung(55_001, date!(2026 - 07 - 10))
                },
                versorgung: Some(versorgung(LieferStatus::Ruhend, None)),
                grid: Some(grid()),
                now: datetime!(2026-07-08 10:00 UTC),
            },
            accepts: evaluate,
            exempt: &[BILANZIERUNGSGEBIET, GRUND_E0622],
        }));
        // E_0622 — Anmeldung erzeugend.
        f.extend(check(&Case {
            name: "E_0622 evaluate (erzeugend)",
            input: Anmeldung {
                anfrage: AnmeldungAnfrage {
                    marktlokationsart: Marktlokationsart::Erzeugend,
                    erzeugung: Some(ErzeugungsAnmeldung {
                        geschaeftsvorfall: Geschaeftsvorfall::Eins,
                        angemeldete_veraeusserungsform: Veraeusserungsform::Marktpraemie,
                        bestehende_veraeusserungsform: Some(Veraeusserungsform::Marktpraemie),
                        nicht_eeg_kwkg: false,
                        ausfallverguetung: false,
                        gewuenschter_prozentsatz: None,
                        tranchen_prozent: std::collections::BTreeMap::new(),
                        direktvermarktungspflichtig: Some(true),
                    }),
                    ..anmeldung(55_077, date!(2026 - 09 - 01))
                },
                versorgung: Some(versorgung(LieferStatus::Grundversorgung, None)),
                grid: Some(grid()),
                now: datetime!(2026-07-08 10:00 UTC),
            },
            accepts: evaluate,
            exempt: &[BILANZIERUNGSGEBIET, GRUND_E0622],
        }));
        // G_0011 — Anmeldung Gas.
        f.extend(check(&Case {
            name: "G_0011 evaluate (Gas)",
            input: Anmeldung {
                anfrage: AnmeldungAnfrage {
                    sparte: Sparte::Gas,
                    ..anmeldung(44_001, date!(2026 - 08 - 01))
                },
                versorgung: Some(versorgung(LieferStatus::Unbeliefert, None)),
                grid: Some(grid()),
                now: datetime!(2026-07-08 10:00 UTC),
            },
            accepts: evaluate,
            exempt: &[BILANZIERUNGSGEBIET, GRUND_G0011],
        }));

        // G_0011 — an Ein-/Auszug inside the Wechsel lead: only the
        // Transaktionsgrund tells it from a Wechsel that is too close.
        f.extend(check(&Case {
            name: "G_0011 evaluate (Gas, E01 inside the Wechsel lead)",
            input: Anmeldung {
                anfrage: AnmeldungAnfrage {
                    sparte: Sparte::Gas,
                    transaktionsgrund: Some("E01".to_owned()),
                    ..anmeldung(44_001, date!(2026 - 07 - 15))
                },
                versorgung: Some(versorgung(LieferStatus::Unbeliefert, None)),
                grid: Some(grid()),
                now: datetime!(2026-07-08 10:00 UTC),
            },
            accepts: evaluate,
            exempt: &[BILANZIERUNGSGEBIET],
        }));

        // E_0607 — Abmeldung.
        #[derive(Serialize, Deserialize)]
        struct Abmeldung {
            anfrage: AbmeldungAnfrage,
            versorgung: Option<VersorgungsStatusRecord>,
            #[serde(with = "time::serde::rfc3339")]
            now: OffsetDateTime,
        }
        f.extend(check(&Case {
            name: "E_0607 evaluate_abmeldung",
            input: Abmeldung {
                anfrage: AbmeldungAnfrage {
                    pid: 55_004,
                    process_id: uuid::Uuid::nil(),
                    malo_id: "51238696012".to_owned(),
                    lf_mp_id: LFN.to_owned(),
                    grid_operator_gln: NB.to_owned(),
                    abmeldedatum: date!(2026 - 04 - 01),
                    sparte: Sparte::Strom,
                    messtyp: Messtyp::Slp,
                    transaktionsgrund: Some("E03".to_owned()),
                    marktlokationsart: Marktlokationsart::Verbrauchend,
                    erzeugung: None,
                },
                versorgung: Some(versorgung(LieferStatus::Beliefert, Some(LFN))),
                now: datetime!(2026-03-02 09:00 UTC),
            },
            accepts: |a| {
                ja(&mako_pruefung::evaluate_abmeldung(
                    &a.anfrage,
                    a.versorgung.as_ref(),
                    a.now,
                    &NetzCheckConfig::default(),
                ))
            },
            exempt: &[],
        }));

        f.extend(check(&Case {
            name: "G_0007 evaluate_abmeldung (Gas, retroactive E01)",
            input: Abmeldung {
                anfrage: AbmeldungAnfrage {
                    pid: 44_004,
                    process_id: uuid::Uuid::nil(),
                    malo_id: "51238696012".to_owned(),
                    lf_mp_id: LFN.to_owned(),
                    grid_operator_gln: NB.to_owned(),
                    abmeldedatum: date!(2026 - 02 - 20),
                    sparte: Sparte::Gas,
                    messtyp: Messtyp::Slp,
                    transaktionsgrund: Some("E01".to_owned()),
                    marktlokationsart: Marktlokationsart::Verbrauchend,
                    erzeugung: None,
                },
                versorgung: Some(versorgung(LieferStatus::Beliefert, Some(LFN))),
                now: datetime!(2026-03-02 09:00 UTC),
            },
            accepts: |a| {
                ja(&mako_pruefung::evaluate_abmeldung(
                    &a.anfrage,
                    a.versorgung.as_ref(),
                    a.now,
                    &NetzCheckConfig::default(),
                ))
            },
            exempt: &[],
        }));

        // E_0623 — Lieferbeginn, with and without an Abmeldeanfrage answered.
        #[derive(Serialize, Deserialize)]
        struct Lieferbeginn {
            anfrage: AnmeldungAnfrage,
        }
        let lieferbeginn =
            |l: &Lieferbeginn| ja(&mako_pruefung::evaluate_lieferbeginn(&l.anfrage, None));
        f.extend(check(&Case {
            name: "E_0623 evaluate_lieferbeginn",
            input: Lieferbeginn {
                anfrage: anmeldung(55_001, date!(2026 - 11 - 01)),
            },
            accepts: lieferbeginn,
            exempt: &[BILANZIERUNGSGEBIET_E0623, GRUND_E0623],
        }));
        f.extend(check(&Case {
            name: "E_0623 evaluate_lieferbeginn (LFA Zustimmung)",
            input: Lieferbeginn {
                anfrage: AnmeldungAnfrage {
                    abmeldeanfrage: Abmeldeanfrage::Gestellt {
                        antwort: Some(LfaAntwort::Zustimmung {
                            code: "A31".to_owned(),
                            zuordnungsende: Some(date!(2026 - 10 - 31)),
                        }),
                    },
                    ..anmeldung(55_001, date!(2026 - 11 - 01))
                },
            },
            accepts: lieferbeginn,
            exempt: &[
                BILANZIERUNGSGEBIET_E0623,
                GRUND_E0623,
                LFA_SCHWEIGT,
                LFA_ZUORDNUNGSENDE,
            ],
        }));

        // E_0608 — Neuanlage.
        #[derive(Serialize, Deserialize)]
        struct Neuanlage {
            anfrage: NeuanlageAnfrage,
            identifikation: Identifikation,
            befund: Option<NeuanlageBefund>,
            today: time::Date,
        }
        f.extend(check(&Case {
            name: "E_0608 evaluate_neuanlage",
            input: Neuanlage {
                anfrage: NeuanlageAnfrage {
                    pid: 55_600,
                    marktlokationsart: Marktlokationsart::Verbrauchend,
                    lf_mp_id: LFN.to_owned(),
                    zuordnungsbeginn: date!(2026 - 03 - 09),
                    uebertragungstag: date!(2026 - 03 - 04),
                    erzeugung: None,
                },
                identifikation: Identifikation::Eindeutig {
                    malo_id: "51238696012".to_owned(),
                },
                befund: Some(NeuanlageBefund {
                    nimmt_an_mako_teil: true,
                    erstmalige_inbetriebnahme: true,
                    lf_bereits_zugeordnet: false,
                    im_netzgebiet: true,
                    anforderungen_erfuellt: true,
                    viertelstundenmessung: true,
                }),
                today: date!(2026 - 03 - 04),
            },
            accepts: |n| {
                matches!(
                    mako_pruefung::nb::neuanlage::evaluate_neuanlage(
                        &n.anfrage,
                        &n.identifikation,
                        n.befund.as_ref(),
                        n.today,
                        &NetzCheckConfig::default(),
                    ),
                    NeuanlageEntscheidung::Accept(_)
                )
            },
            exempt: &[],
        }));

        run_all(f);
    }
}

// ── Rechnungsprüfung (ESA, Preisblatt B) ──────────────────────────────────────

mod rechnung {
    use super::{Case, check, run_all};
    use mako_pruefung::HolidayCalendar;
    use mako_pruefung::rechnung::{
        FAMILIEN, PositionsFakten, RechnungsFakten, Steuersatzpruefung, StornoAntwort,
        StornoFakten, UrsprungsAntwort, Zeitraum, pruefe_rechnung, pruefe_rechnung_erneut,
        pruefe_stornorechnung,
    };
    use time::macros::date;

    const CAL: HolidayCalendar = HolidayCalendar::BdewMaKo;

    fn rechnung() -> RechnungsFakten {
        let maerz = Zeitraum {
            von: date!(2026 - 03 - 01),
            bis: date!(2026 - 03 - 31),
        };
        RechnungsFakten {
            einwaende_entkraeftet: Some(true),
            ustg_konform: Some(true),
            rechnungsdatum: date!(2026 - 04 - 01),
            eingangsdatum: date!(2026 - 04 - 01),
            leistungszeitraum: Some(maerz),
            bestellung_bekannt: true,
            rechnungsnummer_bereits_verwendet: false,
            faelliger_betrag_nicht_negativ: true,
            zahlungsziel: Some(date!(2026 - 05 - 15)),
            preisblatt_version_gueltig: Some(true),
            zeitraum_bereits_abgerechnet_in: None,
            sonstiger_kopffehler: None,
            positionen: vec![PositionsFakten {
                positionsnummer: 1,
                artikel_id: Some("9990001100002".to_owned()),
                artikel_id_aus_bestellung: true,
                leistung_erbracht: Some(true),
                preis_wie_angebot: Some(true),
                steuersatz_korrekt: Some(true),
                zeitraum: Some(maerz),
                bereits_abgerechnet_in: None,
                rechenfehler: false,
                sonstiger_fehler: None,
            }],
            fehlende_artikel_ids: Vec::new(),
            steuersaetze: vec![Steuersatzpruefung {
                steuersatz: "19.00".to_owned(),
                steuerkategorie: "S".to_owned(),
                besteuerungsgrundlage_stimmt: true,
                steuerbetrag_stimmt: true,
            }],
            rechnungsbetrag_stimmt: true,
            sonstiger_summenfehler: None,
        }
    }

    /// A walk's name and its acceptance.
    type Walk = (&'static str, fn(&RechnungsFakten) -> bool);

    #[test]
    fn no_invoice_walk_accepts_an_absent_input() {
        use mako_pruefung::rechnung::{ESA, PREISBLATT_B_LF, PREISBLATT_B_NB};
        let mut f = Vec::new();
        let walks: [Walk; 6] = [
            ("E_0264 pruefe_rechnung", |r| {
                pruefe_rechnung(ESA, r, CAL).ist_zustimmung()
            }),
            ("E_0270 pruefe_rechnung", |r| {
                pruefe_rechnung(PREISBLATT_B_LF, r, CAL).ist_zustimmung()
            }),
            ("E_0273 pruefe_rechnung", |r| {
                pruefe_rechnung(PREISBLATT_B_NB, r, CAL).ist_zustimmung()
            }),
            ("E_0266 pruefe_rechnung_erneut", |r| {
                pruefe_rechnung_erneut(ESA, r, CAL).ist_zustimmung()
            }),
            ("E_0276 pruefe_rechnung_erneut", |r| {
                pruefe_rechnung_erneut(PREISBLATT_B_LF, r, CAL).ist_zustimmung()
            }),
            ("E_0277 pruefe_rechnung_erneut", |r| {
                pruefe_rechnung_erneut(PREISBLATT_B_NB, r, CAL).ist_zustimmung()
            }),
        ];
        for (name, accepts) in walks {
            f.extend(check(&Case {
                name,
                input: rechnung(),
                accepts,
                exempt: &[],
            }));
        }
        f.extend(check(&Case {
            name: "E_0267 pruefe_stornorechnung",
            input: StornoFakten {
                ursprungsrechnung_bekannt: true,
                rechnungsnummer_bereits_verwendet: false,
                ustg_konform: Some(true),
                bereits_storniert: false,
                rechnungstyp_identisch: true,
                zeitraum_identisch: true,
                betraege_negiert_identisch: true,
                sonstiger_fehler: None,
                ursprungsantwort: UrsprungsAntwort::Zugestimmt,
            },
            accepts: |s| matches!(pruefe_stornorechnung(ESA, s), StornoAntwort::Zustimmen),
            exempt: &[],
        }));
        // A new family is a new walk to list above.
        assert_eq!(
            FAMILIEN.len(),
            3,
            "every RechnungsFamilie has its walks here"
        );
        run_all(f);
    }
}

// ── MaBiS Listenabgleich ──────────────────────────────────────────────────────

#[cfg(feature = "role-mabis")]
mod mabis {
    use super::{Case, check, run_all};
    use mako_pruefung::mabis::codes::{
        EBD_BG_CL_ABO, EBD_BG_CL_EINZEL, EBD_DZUE_LISTE, EBD_LF_AACL, EBD_LF_AACL_EINZEL,
        EBD_LF_CL_CLEARING, EBD_LF_CL_EINZEL, EBD_LF_CL_ERSTABO, EBD_LF_CL_FOLGEABO,
    };
    use mako_pruefung::mabis::{ListenEntscheidung, ListenPruefung, pruefe_liste};
    use serde::{Deserialize, Serialize};

    /// The whole-list facts of [`ListenPruefung`], which borrows its positions
    /// and so is not itself deserialisable.
    #[derive(Serialize, Deserialize)]
    struct Liste {
        abonnement_bestellt: Option<bool>,
        zeitraum_plausibel: Option<bool>,
        mabis_zp_passt: Option<bool>,
        innerhalb_clearingphase: Option<bool>,
        version_zugelassen: Option<bool>,
    }

    /// No position disputed and no whole-list refusal: the empty Korrekturliste.
    fn akzeptiert(ebd: &'static str, l: &Liste) -> bool {
        let p = ListenPruefung {
            abonnement_bestellt: l.abonnement_bestellt,
            zeitraum_plausibel: l.zeitraum_plausibel,
            mabis_zp_passt: l.mabis_zp_passt,
            innerhalb_clearingphase: l.innerhalb_clearingphase,
            version_zugelassen: l.version_zugelassen,
            positionen: &[],
        };
        matches!(pruefe_liste(ebd, &p), ListenEntscheidung::Korrekturliste(k) if k.is_empty())
    }

    /// A Listen tree and its acceptance.
    type Walk = (&'static str, fn(&Liste) -> bool);

    #[test]
    fn no_listen_walk_accepts_an_absent_input() {
        let walks: [Walk; 9] = [
            (EBD_LF_CL_ERSTABO, |l| akzeptiert(EBD_LF_CL_ERSTABO, l)),
            (EBD_LF_CL_FOLGEABO, |l| akzeptiert(EBD_LF_CL_FOLGEABO, l)),
            (EBD_LF_CL_EINZEL, |l| akzeptiert(EBD_LF_CL_EINZEL, l)),
            (EBD_LF_CL_CLEARING, |l| akzeptiert(EBD_LF_CL_CLEARING, l)),
            (EBD_LF_AACL, |l| akzeptiert(EBD_LF_AACL, l)),
            (EBD_LF_AACL_EINZEL, |l| akzeptiert(EBD_LF_AACL_EINZEL, l)),
            (EBD_BG_CL_ABO, |l| akzeptiert(EBD_BG_CL_ABO, l)),
            (EBD_BG_CL_EINZEL, |l| akzeptiert(EBD_BG_CL_EINZEL, l)),
            (EBD_DZUE_LISTE, |l| akzeptiert(EBD_DZUE_LISTE, l)),
        ];
        let mut f = Vec::new();
        for (name, accepts) in walks {
            f.extend(check(&Case {
                name,
                input: Liste {
                    abonnement_bestellt: Some(true),
                    zeitraum_plausibel: Some(true),
                    mabis_zp_passt: Some(true),
                    innerhalb_clearingphase: Some(true),
                    version_zugelassen: Some(true),
                },
                accepts,
                exempt: &[],
            }));
        }
        run_all(f);
    }
}
