//! `E_0623` / `E_3007` — **Lieferbeginn prüfen**, the tree that decides the
//! Bestätigung.
//!
//! [`super::anmeldung::evaluate`] walks `E_0622` / `E_3005`, the *Vorprüfung*:
//! every code it publishes is an Ablehnung, and surviving it means only that
//! the Anmeldung is not **directly** refusable. What the NB actually answers
//! comes from this tree, and it is not one code.
//!
//! # The step between the two trees
//!
//! GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 **Prüfschritt 4** sits between
//! them: „Ist die Marktlokation bzw. Tranche zum Zuordnungsbeginn einem LF
//! zugeordnet, fährt der NB mit Prozessschritt 2 fort, ansonsten mit
//! Prozessschritt 5." Prozessschritt 2 is the Information über existierende
//! Zuordnung (55036) and Nr. 3, „parallel zu Nr. 2", is the **Anfrage zur
//! Beendigung der Zuordnung** (55010) the NB owes the incumbent LFA. The LFA
//! answers by 09:00 Uhr des 1. WT — and „verstreicht die Frist, ohne dass eine
//! Antwort beim NB eingeht, gilt dies als Bestätigung nach Fall a)".
//!
//! Only then can `E_0623` run, because Prüfschritte 20–50 read that answer.
//!
//! # The tree
//!
//! ```text
//!  10  verbrauchende/ruhende MaLo?          ja → 20        nein → 400
//!  20  Anfrage zur Beendigung gestellt?     ja → 30        nein → 60
//!  30  LFA fristgerecht geantwortet?        ja → 40        nein → 60   ← Schweigen = Zustimmung
//!  40  LFA widersprochen?                   ja → 50        nein → 60
//!  50  war der Widerspruch A30?             ja → 60        nein → A50  Ablehnung
//!  60  unspezifizierter Fehler?             ja → A99       nein → A51  Zustimmung
//!
//! 400  Geschäftsvorfall 3?                  ja → 500       nein → 410
//! 410  Anfrage gestellt?                    ja → 420       nein → 450
//! 420  LFA fristgerecht geantwortet?        ja → 430       nein → 450
//! 430  LFA widersprochen?                   ja → 440       nein → 450
//! 440  war der Widerspruch A41?             ja → 450       nein → A57  Ablehnung
//! 450  unspezifizierter Fehler?             ja → A99       nein → A58  Zustimmung
//!
//! 500  Anfragen an die Tranchen-LF gestellt?    ja → 510   nein → 600
//! 510  mindestens einer zugestimmt?             ja → 520   nein → A53  Ablehnung
//! 520  ausreichend großer Prozentsatz frei?     ja → 530   nein → A54  Ablehnung
//! 530  verbleibt ein Anteil im BK des NB?       ja → 540   nein → 600
//! 540  direktvermarktungspflichtige MaLo?       ja → A55   nein → 600
//! 600  unspezifizierter Fehler?                 ja → A99   nein → A56  Zustimmung
//! ```
//!
//! Prüfschritte 50 and 440 are the same rule with different codes: a Widerspruch
//! is only an Ablehnung when it is **not** „die Belieferung wurde zum
//! angefragten Termin bereits beendet und eine vom NB bestätigte Abmeldung liegt
//! vor" (`A30` verbrauchend, `A41` Tranche). That answer says the assignment is
//! already ending — which is what the NB wanted — so the Anmeldung is confirmed.
//!
//! # Gas
//!
//! GeLi Gas states the same rule as a flat code rather than a tree: `G_0011`
//! **`Z35`** „Ablehnung der Abmeldeanfrage", whose Erläuterung reads „Dieser
//! Grund wird nur angewendet bei einer Antwort des NB auf die Anmeldung eines
//! LFN, wenn zuvor eine Abmeldeanfrage des NB beim LFA fehlgeschlagen ist."
//! Prüfschritt 50 has no Gas counterpart: `G_0007`, the tree the Gas LFA answers
//! a 44010 from, publishes no „bereits beendet" code to except.
//!
//! # Fundstellen
//!
//! - Entscheidungsbaum-Diagramme und Codelisten **4.3** Kap. 6.6.4 (`E_0623`),
//!   13.6.4 (`E_3007` / `G_0011` `Z35`)
//! - BK6-24-174 GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 Prüfschritt 4, Nr. 3, Nr. 4

use mako_markt::domain::Sparte;

use crate::codes::{AntwortCode, Cluster, EBD_LIEFERBEGINN, EBD_LIEFERBEGINN_GAS, lookup};

use super::types::{
    Abmeldeanfrage, AnmeldungAnfrage, Geschaeftsvorfall, LfaAntwort, NbEntscheidung, RejectReason,
    TranchenLage, TranchenZuordnung,
};

/// `E_0624` Prüfschritt code the LFA uses for „die Belieferung wurde zu dem
/// angefragten Termin bereits beendet und eine vom NB bestätigte Abmeldung liegt
/// vor" — the one Widerspruch that does **not** refuse the Anmeldung
/// (`E_0623` Prüfschritt 50).
pub const BEREITS_ABGEMELDET_VERBRAUCHEND: &str = "A30";

/// The Tranchen twin of [`BEREITS_ABGEMELDET_VERBRAUCHEND`]
/// (`E_0623` Prüfschritt 440).
pub const BEREITS_ABGEMELDET_TRANCHE: &str = "A41";

/// The `E_0623` codes that oblige the NB to **restate the LFA's own ground**.
///
/// `A50` (Prüfschritt 50) and `A57` (440) both mean „der LFA hat der Anfrage zur
/// Beendigung der Zuordnung widersprochen", and GPKE Teil 2 § 2.1.2 Nr. 6 adds:
/// „Der NB gibt zusätzlich den Grund der Ablehnung des LFA an, sofern dieser in
/// Prozessschritt 4 die Anfrage abgelehnt hat."
///
/// On the wire that is `SG4 STS+Z35` „Status der Antwort des dritten
/// Marktbeteiligten", which UTILMD AHB Strom marks Muss alongside these two
/// codes (Bedingungen `[356]` and `[84]`) and alongside no others. `edi-energy`
/// carries the same pair as `utilmd_codes::CODES_REQUIRING_DRITTER` for the
/// render-time guard; `makod` pins the two together, because a domain crate and
/// the wire library cannot depend on each other.
pub const CODES_REQUIRING_DRITTER: &[&str] = &["A50", "A57"];

fn code(ebd: &'static str, c: &'static str) -> &'static AntwortCode {
    lookup(ebd, c).unwrap_or_else(|| panic!("{ebd} publishes {c}"))
}

fn reject(ebd: &'static str, c: &'static str, pruefschritt: u16, detail: String) -> NbEntscheidung {
    NbEntscheidung::Reject(RejectReason::new(ebd, code(ebd, c), pruefschritt, detail))
}

/// Walk `E_0623` / `E_3007` and return the answer the NB owes the LFN.
///
/// Call this **after** [`super::anmeldung::evaluate`] has returned
/// [`NbEntscheidung::Accept`]: this tree assumes the Vorprüfung is behind it.
///
/// `anfrage.abmeldeanfrage` carries what Prüfschritte 20–50 / 410–440 read. When
/// it is [`Abmeldeanfrage::Erforderlich`] — an LFA holds the Marktlokation and
/// the 55010 has not gone out yet — the verdict is
/// [`NbEntscheidung::AnfrageErforderlich`], which is a **process step, not an
/// error**: send the Anfrage, wait for the answer or the 09:00 window, and call
/// again with [`Abmeldeanfrage::Gestellt`].
#[must_use]
pub fn evaluate_lieferbeginn(
    anfrage: &AnmeldungAnfrage,
    tranchen: Option<&TranchenLage>,
) -> NbEntscheidung {
    if anfrage.sparte == Sparte::Gas {
        return g_0012(anfrage);
    }
    // ── 10: verbrauchende oder ruhende Marktlokation? ────────────────────────
    if anfrage.marktlokationsart.ist_verbrauchend_oder_ruhend() {
        return verbrauchend(anfrage);
    }
    // ── 400: Geschäftsvorfall 3? ─────────────────────────────────────────────
    let gv3 = anfrage
        .erzeugung
        .as_ref()
        .is_some_and(|e| e.geschaeftsvorfall == Geschaeftsvorfall::Drei);
    if gv3 {
        return geschaeftsvorfall_3(anfrage, tranchen);
    }
    erzeugend(anfrage)
}

/// Prüfschritte 20–60 — verbrauchende und ruhende Marktlokation.
fn verbrauchend(anfrage: &AnmeldungAnfrage) -> NbEntscheidung {
    match widerspruch(&anfrage.abmeldeanfrage, BEREITS_ABGEMELDET_VERBRAUCHEND) {
        Widerspruch::Anfrage => anfrage_erforderlich(anfrage),
        Widerspruch::Unbekannt(reason) => NbEntscheidung::Escalate { reason },
        // 50 „nein" — the LFA refused for a reason other than „bereits
        // abgemeldet", so the Marktlokation is not free at the Zuordnungsbeginn.
        Widerspruch::Refused(lfa) => reject(
            EBD_LIEFERBEGINN,
            "A50",
            50,
            widerspruch_detail(anfrage, &lfa, BEREITS_ABGEMELDET_VERBRAUCHEND),
        ),
        // 20/30/40 „nein", or 50 „ja" — all four fall through to 60.
        Widerspruch::Frei => {
            NbEntscheidung::accept(EBD_LIEFERBEGINN, code(EBD_LIEFERBEGINN, "A51"))
        }
    }
}

/// Prüfschritte 410–450 — erzeugende Marktlokation, Geschäftsvorfall 1 und 2.
fn erzeugend(anfrage: &AnmeldungAnfrage) -> NbEntscheidung {
    match widerspruch(&anfrage.abmeldeanfrage, BEREITS_ABGEMELDET_TRANCHE) {
        Widerspruch::Anfrage => anfrage_erforderlich(anfrage),
        Widerspruch::Unbekannt(reason) => NbEntscheidung::Escalate { reason },
        Widerspruch::Refused(lfa) => reject(
            EBD_LIEFERBEGINN,
            "A57",
            440,
            widerspruch_detail(anfrage, &lfa, BEREITS_ABGEMELDET_TRANCHE),
        ),
        Widerspruch::Frei => {
            NbEntscheidung::accept(EBD_LIEFERBEGINN, code(EBD_LIEFERBEGINN, "A58"))
        }
    }
}

/// Prüfschritte 500–600 — Geschäftsvorfall 3, the tranchierte Marktlokation.
///
/// A different question from the other two branches: not „did *the* LFA
/// agree?" but „did enough percentage come free?". Several LFA hold Tranchen of
/// one Marktlokation, the NB asks all of them (SD Lieferbeginn Nr. 3, „im Fall
/// von Geschäftsvorfall 3 allen LFA"), and the arithmetic over their answers
/// decides.
fn geschaeftsvorfall_3(
    anfrage: &AnmeldungAnfrage,
    tranchen: Option<&TranchenLage>,
) -> NbEntscheidung {
    // Prüfschritt 500 „wurden Anfragen … gestellt?" presupposes that the
    // Anfrage-Leg has run. While it has not, the answer is the same process
    // step the other two branches return — and for a Geschäftsvorfall 3 it
    // names **every** Tranchen-LFA, „im Fall von Geschäftsvorfall 3 allen LFA"
    // (SD Lieferbeginn Nr. 3). Without this the tree would run the arithmetic
    // over answers nobody has been asked for yet.
    if matches!(anfrage.abmeldeanfrage, Abmeldeanfrage::Erforderlich { .. }) {
        return anfrage_erforderlich(anfrage);
    }
    let Some(lage) = tranchen else {
        return NbEntscheidung::Escalate {
            reason: format!(
                "MaLo {}: Geschäftsvorfall 3 answers out of E_0623 Prüfschritte 500–600, which \
                 read the Tranchen-Zuordnung of the Marktlokation — which LFA hold shares, \
                 which agreed to release them, and what percentage is left in the NB's own \
                 Bilanzkreis. No `TranchenLage` was supplied, and none of A53/A54/A55/A56 \
                 is a safe default: two refuse the Anmeldung and two confirm it.",
                anfrage.malo_id
            ),
        };
    };
    // 420–440 per Tranche. An LFA answering with a code E_0624 does not publish
    // as an Ablehnung escalates the whole Geschäftsvorfall, exactly as it does
    // on the single-LFA branches — the share it holds is neither free nor held.
    let t = &match lage.auswerten() {
        Ok(t) => t,
        Err(reason) => return NbEntscheidung::Escalate { reason },
    };
    // ── 500: Anfragen an die zugeordneten Lieferanten gestellt? ─────────────
    if !t.anfragen_gestellt {
        // „nein → **600**", not „nein → 530": no Tranche was assigned, so
        // nothing had to be released and 530/540 are skipped entirely. Routing
        // through them would let an Anmeldung against an unassigned
        // Marktlokation answer `A55` — the „Herstellung einer 100 %
        // LF-Zuordnung" trigger — on a Marktlokation where no Zuordnung was
        // ended at all.
        return NbEntscheidung::accept(EBD_LIEFERBEGINN, code(EBD_LIEFERBEGINN, "A56"));
    }
    // ── 510: mindestens einer Anfrage zugestimmt? ───────────────────────────
    if !t.mindestens_eine_zustimmung {
        return reject(
            EBD_LIEFERBEGINN,
            "A53",
            510,
            format!(
                "MaLo {}: every LFA refused the Anfrage zur Beendigung der Zuordnung, so none of \
                 the requested {} % came free.",
                anfrage.malo_id, t.gewuenschter_prozentsatz
            ),
        );
    }
    // ── 520: ausreichend großer Prozentsatz frei geworden? ──────────────────
    if !t.ausreichender_prozentsatz {
        return reject(
            EBD_LIEFERBEGINN,
            "A54",
            520,
            format!(
                "MaLo {}: {} % came free, which is less than the {} % the LFN registered.",
                anfrage.malo_id, t.freigewordener_prozentsatz, t.gewuenschter_prozentsatz
            ),
        );
    }
    gv3_zustimmung(t)
}

/// Prüfschritte 530–600 — which Zustimmung a Geschäftsvorfall 3 earns.
fn gv3_zustimmung(t: &TranchenZuordnung) -> NbEntscheidung {
    // 530 „ja" ∧ 540 „ja" → A55; every other path → 600 → A56.
    let c = if t.restanteil_im_nb_bilanzkreis && t.direktvermarktungspflichtig {
        "A55"
    } else {
        "A56"
    };
    NbEntscheidung::accept(EBD_LIEFERBEGINN, code(EBD_LIEFERBEGINN, c))
}

/// `E_3007` — the Gas answer.
///
/// One rule rather than a tree: `G_0011` `Z35` „Ablehnung der Abmeldeanfrage"
/// „wird nur angewendet …, wenn zuvor eine Abmeldeanfrage des NB beim LFA
/// fehlgeschlagen ist". Gas publishes no „bereits abgemeldet" exception, so any
/// Ablehnung from the LFA refuses the Anmeldung.
fn g_0012(anfrage: &AnmeldungAnfrage) -> NbEntscheidung {
    let zustimmung =
        || NbEntscheidung::accept(EBD_LIEFERBEGINN_GAS, code(EBD_LIEFERBEGINN_GAS, "E15"));
    match &anfrage.abmeldeanfrage {
        Abmeldeanfrage::NichtErforderlich => zustimmung(),
        Abmeldeanfrage::Erforderlich { .. } => anfrage_erforderlich(anfrage),
        Abmeldeanfrage::Gestellt { antwort } => match antwort {
            // Silence is consent in Gas too: GeLi Gas 3.0 Kap. 3.2.3 has the
            // GNB proceed when the Abmeldungsanfrage 44010 goes unanswered.
            None | Some(LfaAntwort::Zustimmung { .. }) => zustimmung(),
            Some(LfaAntwort::Widerspruch { code: c, grund }) => reject(
                EBD_LIEFERBEGINN_GAS,
                "Z35",
                0,
                format!(
                    "MaLo {}: the LFA refused the Abmeldungsanfrage with {c}{}. G_0011 Z35 is \
                     the code reserved for exactly this — „wenn zuvor eine Abmeldeanfrage des \
                     NB beim LFA fehlgeschlagen ist\".",
                    anfrage.malo_id,
                    grund
                        .as_ref()
                        .map_or_else(String::new, |g| format!(" ({g})")),
                ),
            ),
        },
    }
}

// ── Prüfschritte 20–50 / 410–440, shared ─────────────────────────────────────

/// The outcome of the Anfrage-leg Prüfschritte, before the code is chosen.
enum Widerspruch {
    /// Prüfschritt 4 of Nr. 1 says an Anfrage is owed and none has gone out.
    Anfrage,
    /// 20/30/40 „nein" or 50/440 „ja" — nothing blocks the Zustimmung.
    Frei,
    /// 50/440 „nein" — the LFA refused, and not with the „bereits abgemeldet"
    /// code. Carries the LFA's answer so the NB can name it.
    Refused(LfaWiderspruch),
    /// The answer arrived but its code is not one `E_0624` publishes, so
    /// Prüfschritt 50 / 440 cannot be decided.
    Unbekannt(String),
}

/// The LFA's refusal, as the NB must restate it.
struct LfaWiderspruch {
    code: String,
    grund: Option<String>,
}

#[allow(clippy::match_same_arms)]
fn widerspruch(anfrage: &Abmeldeanfrage, bereits_abgemeldet: &str) -> Widerspruch {
    // The arms are deliberately not merged: each is a distinct Prüfschritt of
    // `E_0623` reaching the same node, and collapsing them would lose which one
    // a reader is looking at.
    match anfrage {
        // 20 / 410 „nein".
        Abmeldeanfrage::NichtErforderlich => Widerspruch::Frei,
        Abmeldeanfrage::Erforderlich { .. } => Widerspruch::Anfrage,
        // 30 / 420 „nein" — „Verstreicht die Frist, ohne dass eine Antwort beim
        // NB eingeht, gilt dies als Bestätigung nach Fall a)". Silence is the
        // *positive* outcome, so it must not escalate.
        Abmeldeanfrage::Gestellt { antwort: None } => Widerspruch::Frei,
        // 40 / 430 „nein".
        Abmeldeanfrage::Gestellt {
            antwort: Some(LfaAntwort::Zustimmung { .. }),
        } => Widerspruch::Frei,
        Abmeldeanfrage::Gestellt {
            antwort: Some(LfaAntwort::Widerspruch { code: c, grund }),
        } => {
            // 50 / 440 „ja" — „bereits beendet und eine vom NB bestätigte
            // Abmeldung liegt vor" is a refusal of the *Anfrage* and a
            // confirmation of the *Anmeldung*: the assignment is already ending.
            if c == bereits_abgemeldet {
                return Widerspruch::Frei;
            }
            match lookup(crate::codes::EBD_BEENDIGUNG_ZUORDNUNG, c) {
                Some(resolved) if resolved.cluster == Cluster::Ablehnung => {
                    Widerspruch::Refused(LfaWiderspruch {
                        code: c.clone(),
                        grund: grund.clone(),
                    })
                }
                // A Zustimmungscode arriving as a Widerspruch, or a code the
                // tree does not publish: the LFA's message contradicts itself
                // and Prüfschritt 50 has no answer. Refusing the LFN's
                // Anmeldung on it would be § 20 EnWG-unsafe.
                _ => Widerspruch::Unbekannt(format!(
                    "The LFA answered the Anfrage zur Beendigung der Zuordnung with {c:?}, which \
                     E_0624 does not publish as an Ablehnung. E_0623 Prüfschritt 50 / 440 asks \
                     whether the Widerspruch was {bereits_abgemeldet}, and neither answer is \
                     safe for a code the tree does not define."
                )),
            }
        }
    }
}

/// Did this Tranche come free? — Prüfschritte 420–440, per LFA.
///
/// The same reading as [`widerspruch`] applies to one Tranche instead of to the
/// whole Marktlokation: silence is consent, a Zustimmung releases, and `A41`
/// „bereits beendet" is a refusal of the *Anfrage* that still releases the
/// Tranche. The difference is only that `E_0623` then counts the shares rather
/// than branching on the one answer.
///
/// # Errors
///
/// A code `E_0624` does not publish as an Ablehnung, for the reason
/// [`widerspruch`] escalates on it: Prüfschritt 440 has no answer, and refusing
/// the LFN's Anmeldung on it would be § 20 EnWG-unsafe.
// The two `Ok(true)` arms are 420 „nein" and 430 „nein" — distinct Prüfschritte
// reaching the same node, and merging them would lose which one a reader is
// looking at. The same reason `widerspruch` carries this allow.
#[allow(clippy::match_same_arms)]
pub(super) fn tranche_freigeworden(
    antwort: Option<&LfaAntwort>,
    lf_mp_id: &str,
) -> Result<bool, String> {
    match antwort {
        // 420 „nein" — the Frist lapsed, which GPKE makes a Bestätigung.
        None => Ok(true),
        // 430 „nein".
        Some(LfaAntwort::Zustimmung { .. }) => Ok(true),
        Some(LfaAntwort::Widerspruch { code: c, .. }) => {
            // 440 „ja" — the Zuordnung is already ending, so the share is free.
            if c == BEREITS_ABGEMELDET_TRANCHE {
                return Ok(true);
            }
            match lookup(crate::codes::EBD_BEENDIGUNG_ZUORDNUNG, c) {
                Some(resolved) if resolved.cluster == Cluster::Ablehnung => Ok(false),
                _ => Err(format!(
                    "LFA {lf_mp_id} answered the Anfrage zur Beendigung der Zuordnung with \
                     {c:?}, which E_0624 does not publish as an Ablehnung. E_0623 Prüfschritt \
                     440 asks whether the Widerspruch was {BEREITS_ABGEMELDET_TRANCHE}, and \
                     neither answer is safe for a code the tree does not define."
                )),
            }
        }
    }
}

fn anfrage_erforderlich(anfrage: &AnmeldungAnfrage) -> NbEntscheidung {
    let Abmeldeanfrage::Erforderlich { lfa_mp_ids } = &anfrage.abmeldeanfrage else {
        unreachable!("only reached from the Erforderlich arm")
    };
    NbEntscheidung::AnfrageErforderlich {
        lfa_mp_ids: lfa_mp_ids.clone(),
        zuordnungsende: anfrage.process_date,
    }
}

fn widerspruch_detail(
    anfrage: &AnmeldungAnfrage,
    lfa: &LfaWiderspruch,
    bereits_abgemeldet: &str,
) -> String {
    let bedeutung =
        lookup(crate::codes::EBD_BEENDIGUNG_ZUORDNUNG, &lfa.code).map_or("", |c| c.bedeutung);
    format!(
        "MaLo {}: the LFA refused the Anfrage zur Beendigung der Zuordnung with \
         {}:E_0624 ({bedeutung}){} — and not with {bereits_abgemeldet}, the one Widerspruch \
         E_0623 lets the Anmeldung through on. GPKE Teil 2 § 2.1.2 Nr. 6 requires the NB to \
         state the LFA's Grund alongside its own: it rides `SG4 STS+Z35`, „Status der Antwort \
         des dritten Marktbeteiligten\".",
        anfrage.malo_id,
        lfa.code,
        lfa.grund
            .as_ref()
            .map_or_else(String::new, |g| format!(" — {g}")),
    )
}

#[cfg(test)]
#[allow(clippy::fn_params_excessive_bools, clippy::unnecessary_wraps)]
mod tests {
    use super::*;
    use crate::nb::types::{
        ErzeugungsAnmeldung, Marktlokationsart, Messtyp, TranchenAntwort, Veraeusserungsform,
    };
    use rust_decimal::Decimal;
    use time::{Date, Month};
    use uuid::Uuid;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn anfrage(art: Marktlokationsart, abmeldeanfrage: Abmeldeanfrage) -> AnmeldungAnfrage {
        AnmeldungAnfrage {
            pid: if art == Marktlokationsart::Erzeugend {
                55_077
            } else {
                55_001
            },
            process_id: Uuid::nil(),
            malo_id: "51238696781".to_owned(),
            new_supplier_gln: "9900555000005".to_owned(),
            grid_operator_gln: "9900357000004".to_owned(),
            bilanzierungsgebiet: None,
            process_date: d(2026, Month::November, 1),
            sparte: Sparte::Strom,
            messtyp: Messtyp::Slp,
            transaktionsgrund: Some("E03".to_owned()),
            marktlokationsart: art,
            erzeugung: (art == Marktlokationsart::Erzeugend).then_some(ErzeugungsAnmeldung {
                geschaeftsvorfall: Geschaeftsvorfall::Eins,
                angemeldete_veraeusserungsform: Veraeusserungsform::Marktpraemie,
                bestehende_veraeusserungsform: Some(Veraeusserungsform::Marktpraemie),
                nicht_eeg_kwkg: false,
                ausfallverguetung: false,
                // Untranchiert: the Anmeldung is for the whole Marktlokation.
                gewuenschter_prozentsatz: None,
                tranchen_prozent: std::collections::BTreeMap::new(),
                direktvermarktungspflichtig: None,
            }),
            abmeldeanfrage,
        }
    }

    fn gestellt(antwort: Option<LfaAntwort>) -> Abmeldeanfrage {
        Abmeldeanfrage::Gestellt { antwort }
    }

    fn widerspruch(c: &str) -> Option<LfaAntwort> {
        Some(LfaAntwort::Widerspruch {
            code: c.to_owned(),
            grund: Some("Vertragsbindung bis 31.12.2026".to_owned()),
        })
    }

    /// Prüfschritt 20 „nein" — an unassigned Marktlokation is confirmed without
    /// any Anfrage at all, which is the ordinary Einzug.
    #[test]
    fn an_unassigned_marktlokation_is_confirmed_straight_away() {
        let a = anfrage(
            Marktlokationsart::Verbrauchend,
            Abmeldeanfrage::NichtErforderlich,
        );
        let out = evaluate_lieferbeginn(&a, None);
        assert_eq!(out.antwortcode(), Some("A51"));
        assert_eq!(out.ebd(), Some("E_0623"));
    }

    /// Prüfschritt 4 of SD Nr. 1: an assigned Marktlokation cannot be confirmed
    /// until the LFA has been asked, and the verdict is a *process step*, not an
    /// escalation.
    #[test]
    fn an_assigned_marktlokation_demands_the_anfrage_first() {
        let a = anfrage(
            Marktlokationsart::Verbrauchend,
            Abmeldeanfrage::Erforderlich {
                lfa_mp_ids: vec!["9900111000002".to_owned()],
            },
        );
        let out = evaluate_lieferbeginn(&a, None);
        assert!(out.needs_abmeldeanfrage(), "{out:?}");
        assert!(
            !out.is_escalate(),
            "an owed Anfrage is not an operator case"
        );
        let NbEntscheidung::AnfrageErforderlich {
            lfa_mp_ids,
            zuordnungsende,
        } = out
        else {
            unreachable!()
        };
        assert_eq!(lfa_mp_ids, vec!["9900111000002".to_owned()]);
        // Nr. 3 asks for the Zuordnungsbeginn of this Anmeldung.
        assert_eq!(zuordnungsende, d(2026, Month::November, 1));
    }

    /// „Verstreicht die Frist, ohne dass eine Antwort beim NB eingeht, gilt dies
    /// als Bestätigung nach Fall a)" — Prüfschritt 30 „nein" goes to 60, so
    /// silence must confirm rather than time out.
    #[test]
    fn silence_from_the_lfa_confirms_the_anmeldung() {
        let a = anfrage(Marktlokationsart::Verbrauchend, gestellt(None));
        assert_eq!(evaluate_lieferbeginn(&a, None).antwortcode(), Some("A51"));
    }

    #[test]
    fn a_consenting_lfa_confirms_the_anmeldung() {
        let a = anfrage(
            Marktlokationsart::Verbrauchend,
            gestellt(Some(LfaAntwort::Zustimmung {
                code: "A36".to_owned(),
                zuordnungsende: None,
            })),
        );
        assert_eq!(evaluate_lieferbeginn(&a, None).antwortcode(), Some("A51"));
    }

    /// Prüfschritt 50 „nein" — `A50`, an **Ablehnung** out of `E_0623`. The
    /// tree that carries the Zustimmung also carries four refusals.
    #[test]
    fn a_refusing_lfa_refuses_the_anmeldung_with_a50() {
        let a = anfrage(
            Marktlokationsart::Verbrauchend,
            gestellt(widerspruch("A35")),
        );
        let out = evaluate_lieferbeginn(&a, None);
        assert_eq!(out.antwortcode(), Some("A50"));
        assert_eq!(out.ebd(), Some("E_0623"));
        assert!(out.is_reject());
        // GPKE Teil 2 Nr. 6: the NB states the LFA's Grund alongside its own.
        let NbEntscheidung::Reject(r) = out else {
            unreachable!()
        };
        assert!(r.detail.contains("A35"), "{}", r.detail);
        assert!(r.detail.contains("Vertragsbindung bis"), "{}", r.detail);
    }

    /// Prüfschritt 50 „ja" — `A30` is a refusal of the *Anfrage* and a
    /// confirmation of the *Anmeldung*: the assignment is already ending.
    /// Treating every Widerspruch alike would refuse a Lieferantenwechsel that
    /// the Festlegung confirms.
    #[test]
    fn the_already_deregistered_widerspruch_still_confirms() {
        let a = anfrage(
            Marktlokationsart::Verbrauchend,
            gestellt(widerspruch(BEREITS_ABGEMELDET_VERBRAUCHEND)),
        );
        assert_eq!(evaluate_lieferbeginn(&a, None).antwortcode(), Some("A51"));
    }

    /// The erzeugende branch runs the same shape on its own codes — `A57`
    /// against `A50`, and `A41` against `A30`. Crossing them puts a code on the
    /// wire that the branch does not publish.
    #[test]
    fn the_erzeugende_branch_uses_its_own_codes() {
        let refused = anfrage(Marktlokationsart::Erzeugend, gestellt(widerspruch("A39")));
        assert_eq!(
            evaluate_lieferbeginn(&refused, None).antwortcode(),
            Some("A57")
        );

        let excepted = anfrage(
            Marktlokationsart::Erzeugend,
            gestellt(widerspruch(BEREITS_ABGEMELDET_TRANCHE)),
        );
        assert_eq!(
            evaluate_lieferbeginn(&excepted, None).antwortcode(),
            Some("A58")
        );

        // …and `A30`, the verbrauchende exception, is *not* the erzeugende one.
        let crossed = anfrage(
            Marktlokationsart::Erzeugend,
            gestellt(widerspruch(BEREITS_ABGEMELDET_VERBRAUCHEND)),
        );
        assert_eq!(
            evaluate_lieferbeginn(&crossed, None).antwortcode(),
            Some("A57")
        );
    }

    /// A code `E_0624` does not publish as an Ablehnung leaves Prüfschritt 50
    /// undecidable. Refusing the LFN's Anmeldung on it would be the § 20
    /// EnWG-unsafe direction.
    #[test]
    fn an_unpublished_lfa_code_escalates_rather_than_refusing() {
        let a = anfrage(
            Marktlokationsart::Verbrauchend,
            gestellt(widerspruch("A99")),
        );
        let out = evaluate_lieferbeginn(&a, None);
        assert!(out.is_escalate(), "{out:?}");
    }

    // ── Geschäftsvorfall 3 (Prüfschritte 500–600) ────────────────────────────

    fn gv3(abmeldeanfrage: Abmeldeanfrage) -> AnmeldungAnfrage {
        let mut a = anfrage(Marktlokationsart::Erzeugend, abmeldeanfrage);
        if let Some(e) = a.erzeugung.as_mut() {
            e.geschaeftsvorfall = Geschaeftsvorfall::Drei;
        }
        a
    }

    /// A Tranchen-Lage from real per-Tranche answers: the LFN wants 40 %, and
    /// the Marktlokation is held in four 25 % Tranchen.
    fn lage(antworten: [Option<LfaAntwort>; 4]) -> TranchenLage {
        TranchenLage {
            tranchen: antworten
                .into_iter()
                .enumerate()
                .map(|(i, antwort)| TranchenAntwort {
                    lf_mp_id: format!("99999999999{i}"),
                    prozent: Decimal::from(25),
                    antwort,
                })
                .collect(),
            gewuenschter_prozentsatz: Decimal::from(40),
            direktvermarktungspflichtig: false,
        }
    }

    /// `A40` — the Tranchen twin of `A36`. `E_0624` gives Prüfschritte 200–220
    /// their own alphabet (`A39`–`A42`), so a Tranche never answers with the
    /// verbrauchend branch's codes.
    fn zustimmung() -> Option<LfaAntwort> {
        Some(LfaAntwort::Zustimmung {
            code: "A40".to_owned(),
            zuordnungsende: None,
        })
    }

    /// `A39` „Es besteht eine Vertragsbindung (Tranche)" — a refusal that is
    /// neither `A41` nor a Zustimmungscode, so the Tranche stays held.
    fn abgelehnt() -> Option<LfaAntwort> {
        widerspruch("A39")
    }

    /// Prüfschritt 500 presupposes the Anfrage-Leg has run. While it has not,
    /// a Geschäftsvorfall 3 owes the same process step the other branches do —
    /// and it names **every** Tranchen-LFA, not one.
    #[test]
    fn geschaeftsvorfall_3_asks_every_tranchen_lfa_first() {
        let lfa = vec!["9911111111110".to_owned(), "9922222222220".to_owned()];
        let a = gv3(Abmeldeanfrage::Erforderlich {
            lfa_mp_ids: lfa.clone(),
        });
        // Supplied even though a Lage is available: the answers are not in yet,
        // so counting them would read silence as consent before the Frist ran.
        let l = lage([zustimmung(), zustimmung(), zustimmung(), zustimmung()]);
        let out = evaluate_lieferbeginn(&a, Some(&l));
        let NbEntscheidung::AnfrageErforderlich { lfa_mp_ids, .. } = out else {
            panic!("expected AnfrageErforderlich, got {out:?}")
        };
        assert_eq!(lfa_mp_ids, lfa);
    }

    /// Four of the six `E_0623` outcomes exist only on this branch, and two of
    /// them refuse. Deciding a Geschäftsvorfall 3 without the Tranchen
    /// arithmetic would pick between them at random.
    #[test]
    fn geschaeftsvorfall_3_needs_the_tranchen_arithmetic() {
        let a = gv3(gestellt(None));
        assert!(evaluate_lieferbeginn(&a, None).is_escalate());
    }

    /// Prüfschritt 510 — every LFA refused, so nothing came free.
    #[test]
    fn no_lfa_consented_refuses_with_a53() {
        let a = gv3(gestellt(None));
        let l = lage([abgelehnt(), abgelehnt(), abgelehnt(), abgelehnt()]);
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&l)).antwortcode(),
            Some("A53")
        );
    }

    /// Prüfschritt 520 — one 25 % Tranche came free against a 40 % request.
    #[test]
    fn too_little_freed_refuses_with_a54() {
        let a = gv3(gestellt(None));
        let l = lage([zustimmung(), abgelehnt(), abgelehnt(), abgelehnt()]);
        let out = evaluate_lieferbeginn(&a, Some(&l));
        assert_eq!(out.antwortcode(), Some("A54"));
        let NbEntscheidung::Reject(r) = out else {
            unreachable!()
        };
        assert!(r.detail.contains("25"), "{}", r.detail);
        assert!(r.detail.contains("40"), "{}", r.detail);
    }

    /// Silence is consent per Tranche, exactly as it is for a single LFA:
    /// „Verstreicht die Frist, ohne dass eine Antwort beim NB eingeht, gilt
    /// dies als Bestätigung nach Fall a)". Two silent Tranchen free 50 %.
    #[test]
    fn silence_frees_a_tranche() {
        let a = gv3(gestellt(None));
        let l = lage([None, None, abgelehnt(), abgelehnt()]);
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&l)).antwortcode(),
            Some("A56")
        );
    }

    /// `A41` „bereits beendet" refuses the *Anfrage* and still releases the
    /// Tranche — the Prüfschritt-440 exception, counted per share here.
    #[test]
    fn bereits_abgemeldet_frees_the_tranche_it_refuses() {
        let a = gv3(gestellt(None));
        let l = lage([
            widerspruch(BEREITS_ABGEMELDET_TRANCHE),
            widerspruch(BEREITS_ABGEMELDET_TRANCHE),
            abgelehnt(),
            abgelehnt(),
        ]);
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&l)).antwortcode(),
            Some("A56")
        );
    }

    /// A code `E_0624` does not publish as an Ablehnung leaves Prüfschritt 440
    /// undecided for that share, so the whole Geschäftsvorfall escalates rather
    /// than counting the Tranche either way.
    #[test]
    fn an_unpublished_code_on_one_tranche_escalates() {
        let a = gv3(gestellt(None));
        let l = lage([zustimmung(), zustimmung(), widerspruch("ZZZ"), None]);
        assert!(evaluate_lieferbeginn(&a, Some(&l)).is_escalate());
    }

    /// Exactly the requested share coming free is enough — 520 asks for
    /// „ausreichend groß", not „größer".
    #[test]
    fn exactly_the_requested_share_is_enough() {
        let a = gv3(gestellt(None));
        let mut l = lage([zustimmung(), abgelehnt(), abgelehnt(), abgelehnt()]);
        l.gewuenschter_prozentsatz = Decimal::from(25);
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&l)).antwortcode(),
            Some("A56")
        );
    }

    /// Prüfschritt 500 „nein" — no Tranche was assigned, so nothing had to be
    /// released and 510/520 never run. An empty list is that question's „nein",
    /// which is why it must not fall through to `A53`.
    ///
    /// 500 „nein" goes to **600**, skipping 530/540: an Anmeldung against an
    /// unassigned Marktlokation ends no Zuordnung, so it cannot be the trigger
    /// for „Herstellung einer 100 % LF-Zuordnung" that `A55` is — however the
    /// two facts 530 and 540 read happen to stand.
    #[test]
    fn geschaeftsvorfall_3_without_assigned_tranchen_confirms_with_a56() {
        let a = gv3(Abmeldeanfrage::NichtErforderlich);
        let mut l = lage([None, None, None, None]);
        l.tranchen.clear();
        l.direktvermarktungspflichtig = true; // 540 „ja", and 530 would be too
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&l)).antwortcode(),
            Some("A56")
        );
    }

    /// Prüfschritt 530 is the arithmetic the assignment list makes answerable:
    /// what the LFA who kept their Tranchen hold, plus what the LFN registers,
    /// against 100 %. A remainder is what `A55` calls „fehlende Anteile an der
    /// Marktlokation in der Bilanzierung".
    #[test]
    fn a55_needs_an_unassigned_remainder_and_direktvermarktungspflicht() {
        let a = gv3(gestellt(None));
        // Two 25 % Tranchen freed, two kept, LFN registers 50 % →
        // 50 kept + 50 new = 100 %, nothing left in the NB's Bilanzkreis.
        let mut voll = lage([zustimmung(), zustimmung(), abgelehnt(), abgelehnt()]);
        voll.gewuenschter_prozentsatz = Decimal::from(50);
        voll.direktvermarktungspflichtig = true;
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&voll)).antwortcode(),
            Some("A56"),
            "a fully assigned Marktlokation leaves no share in the NB's Bilanzkreis"
        );

        // The LFN takes only 40 of the 50 that came free → 10 % unassigned.
        let mut rest = voll.clone();
        rest.gewuenschter_prozentsatz = Decimal::from(40);
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&rest)).antwortcode(),
            Some("A55")
        );

        // Same remainder, but the Marktlokation is not direktvermarktungs-
        // pflichtig — 540 „nein" → 600.
        let mut nicht_dv = rest.clone();
        nicht_dv.direktvermarktungspflichtig = false;
        assert_eq!(
            evaluate_lieferbeginn(&a, Some(&nicht_dv)).antwortcode(),
            Some("A56")
        );
    }

    // ── Gas ──────────────────────────────────────────────────────────────────

    fn gas(abmeldeanfrage: Abmeldeanfrage) -> AnmeldungAnfrage {
        let mut a = anfrage(Marktlokationsart::Verbrauchend, abmeldeanfrage);
        a.pid = 44_001;
        a.sparte = Sparte::Gas;
        a
    }

    /// Gas states the rule as a flat code, not a tree: `G_0011` `Z35`
    /// „Ablehnung der Abmeldeanfrage". It has no „bereits abgemeldet"
    /// exception, so `A30` — a Strom code — does not rescue a Gas Anmeldung.
    #[test]
    fn gas_refuses_a_failed_abmeldeanfrage_with_z35() {
        let refused = gas(gestellt(widerspruch("E14")));
        let out = evaluate_lieferbeginn(&refused, None);
        assert_eq!(out.antwortcode(), Some("Z35"));
        // The Gas Codelisten are not named in `STS` DE 1131, so a Gas answer
        // carries no EBD on the wire while still belonging to exactly one tree.
        assert_eq!(out.ebd(), None);
        let NbEntscheidung::Reject(r) = out else {
            unreachable!()
        };
        assert_eq!(r.antwort.tree, EBD_LIEFERBEGINN_GAS);

        assert_eq!(
            evaluate_lieferbeginn(&gas(gestellt(None)), None).antwortcode(),
            Some("E15")
        );
        assert_eq!(
            evaluate_lieferbeginn(&gas(Abmeldeanfrage::NichtErforderlich), None).antwortcode(),
            Some("E15")
        );
    }

    /// Every code this tree can answer with resolves in its own EBD and sits in
    /// the cluster the verdict claims — the guard that keeps an Ablehnungscode
    /// off a Bestätigung.
    #[test]
    fn every_outcome_resolves_in_its_own_tree() {
        for (ebd, c, cluster) in [
            (EBD_LIEFERBEGINN, "A50", Cluster::Ablehnung),
            (EBD_LIEFERBEGINN, "A51", Cluster::Zustimmung),
            (EBD_LIEFERBEGINN, "A53", Cluster::Ablehnung),
            (EBD_LIEFERBEGINN, "A54", Cluster::Ablehnung),
            (EBD_LIEFERBEGINN, "A55", Cluster::Zustimmung),
            (EBD_LIEFERBEGINN, "A56", Cluster::Zustimmung),
            (EBD_LIEFERBEGINN, "A57", Cluster::Ablehnung),
            (EBD_LIEFERBEGINN, "A58", Cluster::Zustimmung),
            (EBD_LIEFERBEGINN_GAS, "E15", Cluster::Zustimmung),
            (EBD_LIEFERBEGINN_GAS, "Z35", Cluster::Ablehnung),
        ] {
            let resolved = lookup(ebd, c).unwrap_or_else(|| panic!("{ebd} publishes {c}"));
            assert_eq!(resolved.cluster, cluster, "{ebd} {c}");
        }
    }
}
