//! The NB's deterministic **Abmeldung** checks — `E_0607` (Strom) and
//! `E_3019` / `G_0007` (Gas).
//!
//! A supplier ends a network assignment by sending an Abmeldung (Strom UTILMD
//! **55004**, Gas **44004**). The Netzbetreiber answers with a Bestätigung
//! (55005 / 44005) or an Ablehnung (55006 / 44006), inside the business Frist:
//! 06:00 Uhr des 1. Werktags nach dem ÜT for Strom, Ablauf des dritten Werktags
//! for Gas.
//!
//! Counterpart to [`super::evaluate`], which decides the *Anmeldung*.
//!
//! # Three code spaces, again
//!
//! | Anwendungsfall | Tree | Fristüberschreitung | bereits bestätigt |
//! |---|---|---|---|
//! | Strom | `E_0607` | `A02` | `A09` / `A10` |
//! | Gas | `E_3019` / `G_0007` | `E17` | `Z08` |
//!
//! `A02` is „Vorlauffrist nicht eingehalten" in `E_0607` and „nimmt nicht an
//! der Marktkommunikation teil" in `E_0622`; the Gas tree defines neither. All
//! four Ablehnungscodes of `G_0007` are `E14` / `E17` / `Z08` / `Z14`, so a
//! 44006 carrying `A02`, `A09` or `A10` states a code the counterparty's
//! Codeliste does not contain.
//!
//! # Checks
//!
//! | Prüfschritt | Question | Outcome on failure |
//! |---|---|---|
//! | — | The MaLo is known to this NB | `Escalate` |
//! | 110 | The requesting LF is the assigned Lieferant | `Escalate` |
//! | 50 | Vorlauffrist eingehalten | `A02` (Strom) / `E17` (Gas) |
//! | 100–130 | Kein bereits bestätigtes Lieferende zum selben Datum | `A09` (Strom) / `Z08` (Gas) |
//! | 140 | — | Zustimmung `A11` (Strom) / `E15` (Gas) |
//!
//! Prüfschritte 10–30 (Kundenanlagen-Herauslösung) and 60–90 (ESV-Ende and
//! Aufhebung einer zukünftigen Zuordnung) need Transaktionsgründe and prior
//! process history this projection does not carry; they escalate rather than
//! guess. Escalation is the § 20 EnWG-safe direction: an unfounded Ablehnung
//! keeps a customer bound to a supplier they have left.
//!
//! Prüfschritt 130 is the clearest case. It asks whether the **already
//! confirmed** Abmeldung stated a move-out reason — a fact about an earlier
//! message. `A10` is only correct when it did; when it did not, the tree
//! continues to 140 and *confirms*. Choosing `A10` because the *new* request
//! names an Auszug answers a different question, and does so in the direction
//! that refuses.
//!
//! # Vorlauffrist (Prüfschritt 50)
//!
//! Strom, per GPKE Teil 2 § 2.5.1 SD Prozessschritt 1:
//!
//! - **EEG-Marktlokationen und ihre Tranchen:** the Zuordnungsende must be a
//!   Monatserster, „spätester ÜT liegt 1 Monat vor dem Zuordnungsende"
//!   (§ 21b Abs. 1, § 21c EEG 2023).
//! - **Alle anderen:** „spätester ÜT ist der Tag vor dem letzten WT vor dem
//!   Zuordnungsende" — at least one full Werktag between receipt and the end.
//!
//! Gas, per GeLi Gas 3.0 Kap. 3.2.1: SLP metering may be abgemeldet up to six
//! weeks (plus Bearbeitungsfrist) into the past outside a Lieferantenwechsel;
//! RLM and SMGW-attached metering may not.
//!
//! # Sources
//!
//! - Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 6.3.1 (`E_0607`),
//!   13.4.1 (`E_3019` / `G_0007` / `G_0008`)
//! - BK6-24-174 GPKE Teil 2 § 2.5.1
//! - BK7-24-01-009 GeLi Gas 3.0 Kap. 3.2.1 / 3.2.2

use time::{Date, Duration, OffsetDateTime};

use mako_markt::domain::Sparte;
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};

use crate::codes::{self, AntwortCode, EBD_ABMELDUNG_GAS_NB, EBD_ABMELDUNG_NB};

use super::anmeldung::{GAS_RUECKWIRKUNG_WOCHEN, has_werktag_strictly_between, months_before};
use super::config::NetzCheckConfig;
use super::types::{AbmeldungAnfrage, Marktlokationsart, Messtyp, NbEntscheidung, RejectReason};

/// Transaktionsgründe from which it follows that the Anschlussnutzer moved out
/// (`E_0607` Prüfschritt 120).
///
/// UTILMD SG4 STS DE9013: `E01` Ein-/Auszug (Umzug), `E02` Einzug in
/// Neuanlage / Auszug wegen Stilllegung.
const AUSZUG_GRUENDE: &[&str] = &["E01", "E02"];

/// The tree that governs an Abmeldung of this Sparte.
const fn tree(sparte: Sparte) -> &'static str {
    match sparte {
        Sparte::Strom => EBD_ABMELDUNG_NB,
        Sparte::Gas => EBD_ABMELDUNG_GAS_NB,
    }
}

fn code(sparte: Sparte, code: &'static str) -> &'static AntwortCode {
    let ebd = tree(sparte);
    codes::lookup(ebd, code)
        .unwrap_or_else(|| panic!("{code} is not published by {ebd} — see crate::codes"))
}

/// Refuse with the code the Sparte's own tree publishes for this condition.
fn reject(
    sparte: Sparte,
    strom_code: &'static str,
    gas_code: &'static str,
    pruefschritt: u16,
    detail: String,
) -> NbEntscheidung {
    let c = match sparte {
        Sparte::Strom => strom_code,
        Sparte::Gas => gas_code,
    };
    NbEntscheidung::Reject(RejectReason::new(
        tree(sparte),
        code(sparte, c),
        pruefschritt,
        detail,
    ))
}

/// Run the NB's Abmeldung checks and return a single decision.
///
/// # Parameters
///
/// - `anfrage` — parsed fields from the `de.mako.process.initiated` CloudEvent.
/// - `versorgung` — current supply state from `marktd`. `None` means the MaLo
///   is unknown to this NB.
/// - `now` — current instant, injected by the caller for testability.
/// - `config` — the same tunables [`super::evaluate`] takes.
///
/// # Returns
///
/// [`NbEntscheidung::Accept`] with the Zustimmungscode — dispatch
/// `gpke.lieferende.bestaetigen` / `geli.lieferende.bestaetigen`.
/// [`NbEntscheidung::Reject`] — dispatch the matching `…ablehnen`.
/// [`NbEntscheidung::Escalate`] — an operator decides.
#[must_use]
pub fn evaluate_abmeldung(
    anfrage: &AbmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    now: OffsetDateTime,
    config: &NetzCheckConfig,
) -> NbEntscheidung {
    let sparte = anfrage.sparte;

    // ── The MaLo is known ────────────────────────────────────────────────────
    let Some(vs) = versorgung else {
        return NbEntscheidung::Escalate {
            reason: format!(
                "MaLo {} is unknown to this Netzbetreiber — an Abmeldung for a MaLo \
                 with no supply state cannot be decided from master data.",
                anfrage.malo_id
            ),
        };
    };

    // ── Prüfschritt 110: the requesting LF is the assigned Lieferant ─────────
    //
    // A mismatch is not one of the tree's enumerated Ablehnungsgründe, so it
    // escalates: the projection can lag a legitimate assignment, and a wrongful
    // Ablehnung keeps a customer bound to a supplier they have already left.
    match vs.lf_mp_id.as_deref() {
        Some(lf) if lf == anfrage.lf_mp_id => {}
        Some(other) => {
            return NbEntscheidung::Escalate {
                reason: format!(
                    "MaLo {} is assigned to LF {other}, but the Abmeldung came from {}. \
                     {} Prüfschritt 110 — operator review required.",
                    anfrage.malo_id,
                    anfrage.lf_mp_id,
                    tree(sparte)
                ),
            };
        }
        None => {
            return NbEntscheidung::Escalate {
                reason: format!(
                    "MaLo {} carries no assigned Lieferant (lieferstatus = {}), so the \
                     Abmeldung by {} cannot be matched against one.",
                    anfrage.malo_id, vs.lieferstatus, anfrage.lf_mp_id
                ),
            };
        }
    }

    // ── Prüfschritt 50: Vorlauffrist ─────────────────────────────────────────
    let today = mako_fristen::berlin_date(now);
    if let Some(violation) = check_vorlauffrist(anfrage, today, *config) {
        return violation;
    }

    // ── Prüfschritte 100–130: kein bereits bestätigtes Lieferende ────────────
    //
    // `lieferende` is only set once the NB has confirmed one, so its presence
    // *is* the „bereits bestätigt" condition, and `lieferstatus` distinguishes a
    // settled end from a running supply.
    if vs.lieferstatus == LieferStatus::Unbeliefert && vs.lieferende == Some(anfrage.abmeldedatum) {
        let neue_meldung_nennt_auszug = anfrage
            .transaktionsgrund
            .as_deref()
            .is_some_and(|g| AUSZUG_GRUENDE.contains(&g));
        if neue_meldung_nennt_auszug {
            // Prüfschritt 130 asks about the *already confirmed* Abmeldung's
            // Transaktionsgrund, which the projection does not keep. `A10` and
            // „continue to 140 and confirm" are both live outcomes, so choosing
            // either would be a guess — and one of them refuses.
            return NbEntscheidung::Escalate {
                reason: format!(
                    "MaLo {}: a Lieferende zum {} is already confirmed and the new Abmeldung \
                     names an Auszugsgrund ({:?}). {} Prüfschritt 130 decides between A10 and \
                     a Bestätigung on the *earlier* message's Transaktionsgrund, which is not \
                     in the supply projection — operator review required.",
                    anfrage.malo_id,
                    anfrage.abmeldedatum,
                    anfrage.transaktionsgrund,
                    tree(sparte)
                ),
            };
        }
        return reject(
            sparte,
            "A09",
            "Z08",
            120,
            format!(
                "Lieferende zum Abmeldedatum {} wurde bereits bestätigt.",
                anfrage.abmeldedatum
            ),
        );
    }

    // ── Prüfschritt 140: Zustimmung ──────────────────────────────────────────
    NbEntscheidung::accept(
        tree(sparte),
        code(
            sparte,
            if sparte == Sparte::Strom {
                "A11"
            } else {
                "E15"
            },
        ),
    )
}

/// Prüfschritt 50 — „Wurde die Vorlauffrist eingehalten?" `None` = eingehalten.
fn check_vorlauffrist(
    anfrage: &AbmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NbEntscheidung> {
    match anfrage.sparte {
        Sparte::Strom => check_vorlauffrist_strom(anfrage, today, config),
        Sparte::Gas => check_vorlauffrist_gas(anfrage, today, config),
    }
}

fn check_vorlauffrist_strom(
    anfrage: &AbmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NbEntscheidung> {
    let d = anfrage.abmeldedatum;

    // EEG-Marktlokationen und Tranchen davon: Monatserster, spätester ÜT liegt
    // einen Monat vor dem Zuordnungsende (GPKE Teil 2 § 2.5.1 SD Nr. 1).
    if anfrage.marktlokationsart == Marktlokationsart::Erzeugend {
        if d.day() != 1 {
            return Some(reject(
                Sparte::Strom,
                "A02",
                "E17",
                50,
                format!(
                    "Abmeldung einer EEG-Marktlokation zum {d}, das kein Monatserster ist. \
                     Das Zuordnungsende muss ein Monatserster sein (§ 21b Abs. 1 EEG 2023; \
                     GPKE Teil 2 § 2.5.1 SD Lieferende von LF an NB, Prozessschritt 1).",
                ),
            ));
        }
        let latest_ut = months_before(d, config.eeg_zuordnung_vorlauf_monate);
        return (today > latest_ut).then(|| {
            reject(
                Sparte::Strom,
                "A02",
                "E17",
                50,
                format!(
                    "Vorlauffrist nicht eingehalten: Abmeldung einer EEG-Marktlokation zum \
                     {d}, spätester ÜT {latest_ut} (ein Monat vor dem Zuordnungsende; \
                     Eingang {today}).",
                ),
            )
        });
    }

    // Alle anderen: „spätester ÜT ist der Tag vor dem letzten WT vor dem
    // Zuordnungsende" — at least one full Werktag between receipt and the end.
    (!has_werktag_strictly_between(today, d, config.holiday_calendar)).then(|| {
        reject(
            Sparte::Strom,
            "A02",
            "E17",
            50,
            format!(
                "Vorlauffrist nicht eingehalten: Zuordnungsende {d} bei Eingang {today}. \
                 Spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsende \
                 (GPKE Teil 2 § 2.5.1 SD Lieferende von LF an NB, Prozessschritt 1) — \
                 mindestens ein voller Werktag muss dazwischen liegen.",
            ),
        )
    })
}

fn check_vorlauffrist_gas(
    anfrage: &AbmeldungAnfrage,
    today: Date,
    config: NetzCheckConfig,
) -> Option<NbEntscheidung> {
    let d = anfrage.abmeldedatum;
    if d >= today {
        return None; // future or same-day Abmeldung is always plausible
    }

    // Lieferantenwechsel: „Wechsel sind nur in die Zukunft gerichtet möglich"
    // (GeLi Gas 3.0 Kap. 3.2.1).
    if anfrage.transaktionsgrund.as_deref() == Some("E03") {
        return Some(reject(
            Sparte::Gas,
            "A02",
            "E17",
            50,
            format!(
                "Rückwirkende Gas-Abmeldung zum {d} (Eingang {today}) anlässlich eines \
                 Lieferantenwechsels — Wechsel sind nur in die Zukunft gerichtet möglich \
                 (GeLi Gas 3.0 Kap. 3.2.1).",
            ),
        ));
    }

    match anfrage.messtyp {
        Messtyp::Slp => {
            let window_end = mako_fristen::add_werktage(
                d.saturating_add(Duration::weeks(GAS_RUECKWIRKUNG_WOCHEN)),
                config.gas_bearbeitungsfrist_wt,
                config.holiday_calendar,
            );
            (today > window_end).then(|| {
                reject(
                    Sparte::Gas,
                    "A02",
                    "E17",
                    50,
                    format!(
                        "Rückwirkende Gas-Abmeldung zum {d} liegt außerhalb des \
                         6-Wochen-Fensters (+{} WT Bearbeitungsfrist, Ende {window_end}; \
                         Eingang {today}) — GeLi Gas 3.0 Kap. 3.2.1 lit. a.",
                        config.gas_bearbeitungsfrist_wt
                    ),
                )
            })
        }
        // „Für Letztverbraucher mit registrierender Leistungsmessung, sowie für
        // neue Messeinrichtungen, die an ein Smart-Meter-Gateway angeschlossen
        // sind, können An- und Abmeldedatum nur nach dem Eingangsdatum liegen."
        Messtyp::Rlm | Messtyp::Imsys => Some(reject(
            Sparte::Gas,
            "A02",
            "E17",
            50,
            format!(
                "Rückwirkende Gas-Abmeldung zum {d} ist bei {}-Messung nicht zulässig — \
                 An- und Abmeldedatum können nur nach dem Eingangsdatum liegen \
                 (GeLi Gas 3.0 Kap. 3.2.1).",
                anfrage.messtyp
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Month, macros::datetime};
    use uuid::Uuid;

    const OWN_LF: &str = "9900357000004";

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    fn anfrage(abmeldedatum: Date) -> AbmeldungAnfrage {
        AbmeldungAnfrage {
            pid: 55_004,
            process_id: Uuid::new_v4(),
            malo_id: "51238696012".to_owned(),
            lf_mp_id: OWN_LF.to_owned(),
            grid_operator_gln: "9900000000002".to_owned(),
            abmeldedatum,
            sparte: Sparte::Strom,
            messtyp: Messtyp::Slp,
            transaktionsgrund: Some("E03".to_owned()),
            marktlokationsart: Marktlokationsart::Verbrauchend,
            erzeugung: None,
        }
    }

    fn versorgung(
        lieferstatus: LieferStatus,
        lf_mp_id: Option<&str>,
        lieferende: Option<Date>,
    ) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: "51238696012".parse().expect("valid MaLo"),
            lieferstatus,
            lf_mp_id: lf_mp_id.map(ToOwned::to_owned),
            lf_mp_id_next: None,
            lf_next_lieferbeginn: None,
            lieferbeginn: None,
            lieferende,
            msb_mp_id: None,
            nb_mp_id: "9900000000002".to_owned(),
            eog_seit: None,
            last_process_id: None,
            updated_at: OffsetDateTime::now_utc(),
            tenant: "9900000000002".to_owned(),
            version: 1,
        }
    }

    // Monday 2026-03-02 09:00 UTC.
    const NOW: OffsetDateTime = datetime!(2026-03-02 09:00 UTC);

    fn cfg() -> NetzCheckConfig {
        NetzCheckConfig::default()
    }

    #[test]
    fn a_clean_future_abmeldung_is_accepted() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let r = evaluate_abmeldung(&anfrage(d(2026, Month::April, 1)), Some(&vs), NOW, &cfg());
        assert!(r.is_accept(), "{r:?}");
    }

    #[test]
    fn an_unknown_malo_escalates_rather_than_rejecting() {
        let r = evaluate_abmeldung(&anfrage(d(2026, Month::April, 1)), None, NOW, &cfg());
        assert!(r.is_escalate());
    }

    /// A wrongful Ablehnung keeps a customer bound to a supplier they have
    /// left, so an LF mismatch is an operator decision, never an auto-reject.
    #[test]
    fn a_different_assigned_lf_escalates() {
        let vs = versorgung(LieferStatus::Beliefert, Some("9900999000001"), None);
        let r = evaluate_abmeldung(&anfrage(d(2026, Month::April, 1)), Some(&vs), NOW, &cfg());
        assert!(r.is_escalate());
    }

    /// Prüfschritt 50: at least one full Werktag between receipt and the
    /// Zuordnungsende. Monday receipt, Tuesday end leaves none.
    #[test]
    fn a_next_day_abmeldung_misses_the_vorlauffrist() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let r = evaluate_abmeldung(&anfrage(d(2026, Month::March, 3)), Some(&vs), NOW, &cfg());
        assert_eq!(r.antwortcode(), Some("A02"), "{r:?}");
    }

    /// The Vorlauffrist code is `A02` in E_0607 — the same string means
    /// something else in the Anmeldung tree, so this pins the code space.
    #[test]
    fn the_vorlauffrist_code_is_a02_not_a07() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let r = evaluate_abmeldung(&anfrage(d(2026, Month::March, 2)), Some(&vs), NOW, &cfg());
        assert_eq!(r.antwortcode(), Some("A02"));
        assert_ne!(r.antwortcode(), Some("A07"));
    }

    #[test]
    fn a_settled_lieferende_at_the_same_date_is_a09() {
        let ende = d(2026, Month::April, 1);
        let vs = versorgung(LieferStatus::Unbeliefert, Some(OWN_LF), Some(ende));
        let r = evaluate_abmeldung(&anfrage(ende), Some(&vs), NOW, &cfg());
        assert_eq!(r.antwortcode(), Some("A09"), "{r:?}");
    }

    /// Prüfschritt 130 turns on the **already confirmed** Abmeldung's
    /// Transaktionsgrund, which the supply projection does not keep. `A10` and
    /// „continue to 140 and confirm" are both live outcomes there, so the
    /// decision escalates instead of picking the one that refuses.
    #[test]
    fn a_repeated_auszug_at_the_same_date_escalates_rather_than_guessing_a10() {
        let ende = d(2026, Month::April, 1);
        let vs = versorgung(LieferStatus::Unbeliefert, Some(OWN_LF), Some(ende));
        let mut a = anfrage(ende);
        a.transaktionsgrund = Some("E01".to_owned());
        let r = evaluate_abmeldung(&a, Some(&vs), NOW, &cfg());
        assert!(r.is_escalate(), "{r:?}");
    }

    /// Gas answers the same conditions out of `G_0007`, whose four codes are
    /// `E14` / `E17` / `Z08` / `Z14`. `A02`, `A09` and `A10` are not in it.
    #[test]
    fn gas_never_answers_with_a_strom_abmeldung_code() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut late = anfrage(d(2026, Month::February, 1));
        late.pid = 44_004;
        late.sparte = Sparte::Gas;
        assert_eq!(
            evaluate_abmeldung(&late, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("E17"),
            "not A02"
        );

        let ende = d(2026, Month::April, 1);
        let settled = versorgung(LieferStatus::Unbeliefert, Some(OWN_LF), Some(ende));
        let mut dup = anfrage(ende);
        dup.pid = 44_004;
        dup.sparte = Sparte::Gas;
        assert_eq!(
            evaluate_abmeldung(&dup, Some(&settled), NOW, &cfg()).antwortcode(),
            Some("Z08"),
            "not A09"
        );
    }

    /// A Bestätigung states its code: `A11` out of `E_0607`, `E15` out of
    /// `G_0008`. `SG4 STS+E01` is Muss on every Antwortnachricht.
    #[test]
    fn the_bestaetigung_states_a_code() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let r = evaluate_abmeldung(&anfrage(d(2026, Month::April, 1)), Some(&vs), NOW, &cfg());
        assert_eq!(r.antwortcode(), Some("A11"));
        assert_eq!(r.ebd(), Some("E_0607"));

        let mut gas = anfrage(d(2026, Month::April, 1));
        gas.pid = 44_004;
        gas.sparte = Sparte::Gas;
        let r = evaluate_abmeldung(&gas, Some(&vs), NOW, &cfg());
        assert_eq!(r.antwortcode(), Some("E15"));
        assert_eq!(r.ebd(), None);
    }

    /// A confirmed Lieferende at a *different* date must not block a new one.
    #[test]
    fn a_lieferende_at_another_date_does_not_block() {
        let vs = versorgung(
            LieferStatus::Unbeliefert,
            Some(OWN_LF),
            Some(d(2026, Month::January, 1)),
        );
        let r = evaluate_abmeldung(&anfrage(d(2026, Month::April, 1)), Some(&vs), NOW, &cfg());
        assert!(r.is_accept(), "{r:?}");
    }

    // ── EEG-Marktlokation ─────────────────────────────────────────────────────

    #[test]
    fn an_eeg_abmeldung_must_be_a_monatserster() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::May, 15));
        a.marktlokationsart = Marktlokationsart::Erzeugend;
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A02")
        );
    }

    #[test]
    fn an_eeg_abmeldung_needs_a_month_of_lead() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::March, 1));
        a.marktlokationsart = Marktlokationsart::Erzeugend;
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A02"),
            "1 March is inside the receipt month"
        );

        // 1 April is one day short too: „spätester ÜT liegt 1 Monat vor dem
        // Zuordnungsende" makes 1 March the latest ÜT, and receipt is 2 March.
        let mut late = anfrage(d(2026, Month::April, 1));
        late.marktlokationsart = Marktlokationsart::Erzeugend;
        assert_eq!(
            evaluate_abmeldung(&late, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A02")
        );

        let mut ok = anfrage(d(2026, Month::May, 1));
        ok.marktlokationsart = Marktlokationsart::Erzeugend;
        assert!(evaluate_abmeldung(&ok, Some(&vs), NOW, &cfg()).is_accept());
    }

    // ── Gas ───────────────────────────────────────────────────────────────────

    #[test]
    fn a_retroactive_gas_wechsel_abmeldung_is_rejected() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::February, 1));
        a.pid = 44_004;
        a.sparte = Sparte::Gas;
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("E17")
        );
    }

    #[test]
    fn a_retroactive_gas_auszug_within_six_weeks_is_accepted_for_slp() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::February, 1));
        a.pid = 44_004;
        a.sparte = Sparte::Gas;
        a.transaktionsgrund = Some("E01".to_owned());
        assert!(evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).is_accept());
    }

    #[test]
    fn a_retroactive_gas_abmeldung_is_never_allowed_for_rlm() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::February, 1));
        a.pid = 44_004;
        a.sparte = Sparte::Gas;
        a.transaktionsgrund = Some("E01".to_owned());
        a.messtyp = Messtyp::Rlm;
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("E17")
        );
    }

    /// Gas has no „ein voller Werktag" rule — a next-day Abmeldung is fine.
    #[test]
    fn a_future_gas_abmeldung_needs_no_werktag_gap() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::March, 3));
        a.pid = 44_004;
        a.sparte = Sparte::Gas;
        assert!(evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).is_accept());
    }
}
