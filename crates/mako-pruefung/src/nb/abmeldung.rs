//! The NB's deterministic **Abmeldung** checks — EBD `E_0607` "Abmeldung
//! prüfen".
//!
//! A supplier ends a network assignment by sending an Abmeldung (Strom UTILMD
//! **55004**, Gas **44004**). The Netzbetreiber answers with a Bestätigung
//! (55005 / 44005) or an Ablehnung (55006 / 44006), inside the business Frist:
//! 06:00 Uhr des 1. Werktags nach dem ÜT for Strom, Ablauf des dritten Werktags
//! for Gas.
//!
//! Counterpart to [`super::evaluate`], which decides the *Anmeldung*. They are
//! separate functions because the trees have **separate code spaces**: `A02` is
//! „Marktlokation nimmt nicht an der Marktkommunikation teil" in `E_0622` and
//! „Vorlauffrist nicht eingehalten" in `E_0607`. Reusing the Anmeldung codes
//! here puts a valid-looking but wrong Ablehnungsgrund on the market.
//!
//! # Checks
//!
//! | # | Prüfschritt (`E_0607`) | Outcome on failure |
//! |---|---|---|
//! | 1 | The MaLo is known to this NB | `Escalate` |
//! | 2 | The requesting LF is the assigned Lieferant | `Escalate` |
//! | 3 | Vorlauffrist eingehalten (Prüfschritt 50) | `Reject A02` |
//! | 4 | Kein bereits bestätigtes Lieferende zum selben Datum (Prüfschritte 100–130) | `Reject A09` / `A10` |
//!
//! Prüfschritte 10–30 (Kundenanlagen-Herauslösung) and 60–90 (ESV-Ende and
//! Aufhebung einer zukünftigen Zuordnung) need Transaktionsgründe and prior
//! process history this projection does not carry; they escalate rather than
//! guess. Escalation is the § 20 EnWG-safe direction: an unfounded Ablehnung
//! keeps a customer bound to a supplier they have left.
//!
//! # Vorlauffrist (Prüfschritt 50)
//!
//! Strom, per GPKE Teil 2 § 2.5.1 SD Prozessschritt 1:
//!
//! - **EEG-Marktlokationen und ihre Tranchen:** the Zuordnungsende must be a
//!   Monatserster, „spätester ÜT liegt 1 Monat vor dem Zuordnungsende".
//! - **Alle anderen:** „spätester ÜT ist der Tag vor dem letzten WT vor dem
//!   Zuordnungsende" — at least one full Werktag between receipt and the end.
//!
//! Gas, per GeLi Gas 3.0 Kap. 3.2.1: SLP metering may be abgemeldet up to six
//! weeks (plus Bearbeitungsfrist) into the past outside a Lieferantenwechsel;
//! RLM and SMGW-attached metering may not.
//!
//! # Sources
//!
//! - Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 6.3.1 (`E_0607`)
//! - BK6-24-174 GPKE Teil 2 § 2.5.1
//! - BK7-24-01-009 GeLi Gas 3.0 Kap. 3.2.1 / 3.2.2

use time::{Date, Duration, OffsetDateTime};

use mako_markt::domain::Sparte;
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};

use super::anmeldung::{GAS_RUECKWIRKUNG_WOCHEN, has_werktag_strictly_between, today_berlin};
use super::config::NetzCheckConfig;
use super::types::{AbmeldungAnfrage, Messtyp, NbEntscheidung, RejectReason};

/// `A02` — Vorlauffrist nicht eingehalten (`E_0607` Prüfschritt 50).
///
/// **Not** the Anmeldung's `A02`: in `E_0622` that code means the MaLo does not
/// take part in market communication.
const ERC_VORLAUFFRIST: &str = "A02";
/// `A09` — Lieferende zum Abmeldedatum wurde bereits bestätigt
/// (`E_0607` Prüfschritt 120).
const ERC_ALREADY_CONFIRMED: &str = "A09";
/// `A10` — Lieferende zum Abmeldedatum wurde aus gleichem Grund bereits
/// bestätigt (`E_0607` Prüfschritt 130).
const ERC_ALREADY_CONFIRMED_SAME_REASON: &str = "A10";

/// Transaktionsgründe from which it follows that the Anschlussnutzer moved out
/// (`E_0607` Prüfschritt 120).
///
/// UTILMD SG4 STS DE9013: `E01` Ein-/Auszug (Umzug), `E02` Einzug in
/// Neuanlage / Auszug wegen Stilllegung.
const AUSZUG_GRUENDE: &[&str] = &["E01", "E02"];

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
/// [`NbEntscheidung::Accept`] — dispatch `gpke.lieferende.bestaetigen` /
/// `geli.lieferende.bestaetigen`.
/// [`NbEntscheidung::Reject`] — dispatch the matching `…ablehnen` with
/// `reason.antwortcode`.
/// [`NbEntscheidung::Escalate`] — an operator decides.
#[must_use]
pub fn evaluate_abmeldung(
    anfrage: &AbmeldungAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    now: OffsetDateTime,
    config: &NetzCheckConfig,
) -> NbEntscheidung {
    // ── Check 1: the MaLo is known ───────────────────────────────────────────
    let Some(vs) = versorgung else {
        return NbEntscheidung::Escalate {
            reason: format!(
                "MaLo {} is unknown to this Netzbetreiber — an Abmeldung for a MaLo \
                 with no supply state cannot be decided from master data.",
                anfrage.malo_id
            ),
        };
    };

    // ── Check 2: the requesting LF is the assigned Lieferant ─────────────────
    //
    // `E_0607` Prüfschritt 110 asks whether the requesting LF is still assigned
    // on the day after the Abmeldedatum. A mismatch is not one of the tree's
    // enumerated Ablehnungsgründe, so it escalates: the projection can lag a
    // legitimate assignment, and a wrongful Ablehnung keeps a customer bound to
    // a supplier they have already left (§ 20 EnWG).
    match vs.lf_mp_id.as_deref() {
        Some(lf) if lf == anfrage.lf_mp_id => {}
        Some(other) => {
            return NbEntscheidung::Escalate {
                reason: format!(
                    "MaLo {} is assigned to LF {other}, but the Abmeldung came from \
                     {}. E_0607 Prüfschritt 110 — operator review required.",
                    anfrage.malo_id, anfrage.lf_mp_id
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

    // ── Check 3: Vorlauffrist (Prüfschritt 50) ───────────────────────────────
    let today = today_berlin(now);
    if let Some(violation) = check_vorlauffrist(anfrage, today, *config) {
        return violation;
    }

    // ── Check 4: no Lieferende already confirmed for the same date ───────────
    //
    // Prüfschritte 100–130. `lieferende` is only set once the NB has confirmed
    // one, so its presence *is* the "bereits bestätigt" condition, and
    // `lieferstatus` distinguishes a settled end from a running supply.
    if vs.lieferstatus == LieferStatus::Unbeliefert && vs.lieferende == Some(anfrage.abmeldedatum) {
        // Prüfschritt 120/130 split on whether the *new* request states a
        // move-out and the confirmed one did not. Without the earlier request's
        // Transaktionsgrund in the projection, only the request's own grund is
        // known: a move-out reason is the case the tree lets through to A10,
        // everything else stops at A09.
        let is_auszug = anfrage
            .transaktionsgrund
            .as_deref()
            .is_some_and(|g| AUSZUG_GRUENDE.contains(&g));
        let (antwortcode, detail) = if is_auszug {
            (
                ERC_ALREADY_CONFIRMED_SAME_REASON,
                format!(
                    "Lieferende zum Abmeldedatum {} wurde bereits bestätigt; die neue \
                     Abmeldung nennt mit {:?} erneut einen Auszugsgrund (EBD E_0607 \
                     Prüfschritt 130 → A10).",
                    anfrage.abmeldedatum, anfrage.transaktionsgrund
                ),
            )
        } else {
            (
                ERC_ALREADY_CONFIRMED,
                format!(
                    "Lieferende zum Abmeldedatum {} wurde bereits bestätigt (EBD E_0607 \
                     Prüfschritt 120 → A09).",
                    anfrage.abmeldedatum
                ),
            )
        };
        return NbEntscheidung::Reject(RejectReason {
            antwortcode: antwortcode.to_owned(),
            ebd: Some("E_0607".into()),
            detail,
            check_number: 4,
        });
    }

    NbEntscheidung::Accept
}

/// `E_0607` Prüfschritt 50 — „Wurde die Vorlauffrist eingehalten?"
///
/// `None` = eingehalten.
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
    if anfrage.ist_erzeugende_marktlokation {
        if d.day() != 1 {
            return Some(NbEntscheidung::Reject(RejectReason {
                antwortcode: ERC_VORLAUFFRIST.to_owned(),
                ebd: Some("E_0607".into()),
                detail: format!(
                    "Abmeldung einer EEG-Marktlokation zum {d}, das kein Monatserster ist. \
                     „Das Zuordnungsende muss ein Monatserster sein\" (GPKE Teil 2 § 2.5.1 \
                     SD Lieferende von LF an NB, Prozessschritt 1) — EBD E_0607 \
                     Prüfschritt 50 → A02.",
                ),
                check_number: 3,
            }));
        }
        let earliest =
            super::anmeldung::first_of_month_after(today, config.eeg_zuordnung_vorlauf_monate);
        return (d < earliest).then(|| {
            NbEntscheidung::Reject(RejectReason {
                antwortcode: ERC_VORLAUFFRIST.to_owned(),
                ebd: Some("E_0607".into()),
                detail: format!(
                    "Vorlauffrist nicht eingehalten: Abmeldung einer EEG-Marktlokation zum \
                     {d}, frühestmöglicher Monatserster ist {earliest} (spätester ÜT liegt \
                     einen Monat vor dem Zuordnungsende; Eingang {today}). EBD E_0607 \
                     Prüfschritt 50 → A02.",
                ),
                check_number: 3,
            })
        });
    }

    // Alle anderen: „spätester ÜT ist der Tag vor dem letzten WT vor dem
    // Zuordnungsende" — at least one full Werktag between receipt and the end.
    if has_werktag_strictly_between(today, d, config.holiday_calendar) {
        return None;
    }
    Some(NbEntscheidung::Reject(RejectReason {
        antwortcode: ERC_VORLAUFFRIST.to_owned(),
        ebd: Some("E_0607".into()),
        detail: format!(
            "Vorlauffrist nicht eingehalten: Zuordnungsende {d} bei Eingang {today}. \
             „Spätester ÜT ist der Tag vor dem letzten WT vor dem Zuordnungsende\" \
             (GPKE Teil 2 § 2.5.1 SD Lieferende von LF an NB, Prozessschritt 1) — \
             mindestens ein voller Werktag muss dazwischen liegen. EBD E_0607 \
             Prüfschritt 50 → A02.",
        ),
        check_number: 3,
    }))
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
        return Some(NbEntscheidung::Reject(RejectReason {
            antwortcode: ERC_VORLAUFFRIST.to_owned(),
            ebd: Some("E_0607".into()),
            detail: format!(
                "Rückwirkende Gas-Abmeldung zum {d} (Eingang {today}) anlässlich eines \
                 Lieferantenwechsels — Wechsel sind nur in die Zukunft gerichtet möglich \
                 (GeLi Gas 3.0 Kap. 3.2.1). EBD E_0607 Prüfschritt 50 → A02.",
            ),
            check_number: 3,
        }));
    }

    match anfrage.messtyp {
        Messtyp::Slp => {
            let window_end = mako_fristen::add_werktage(
                d.saturating_add(Duration::weeks(GAS_RUECKWIRKUNG_WOCHEN)),
                config.gas_bearbeitungsfrist_wt,
                config.holiday_calendar,
            );
            (today > window_end).then(|| {
                NbEntscheidung::Reject(RejectReason {
                    antwortcode: ERC_VORLAUFFRIST.to_owned(),
                    ebd: Some("E_0607".into()),
                    detail: format!(
                        "Rückwirkende Gas-Abmeldung zum {d} liegt außerhalb des \
                         6-Wochen-Fensters (+{} WT Bearbeitungsfrist, Ende {window_end}; \
                         Eingang {today}) — GeLi Gas 3.0 Kap. 3.2.1 lit. a. EBD E_0607 \
                         Prüfschritt 50 → A02.",
                        config.gas_bearbeitungsfrist_wt
                    ),
                    check_number: 3,
                })
            })
        }
        // „Für Letztverbraucher mit registrierender Leistungsmessung, sowie für
        // neue Messeinrichtungen, die an ein Smart-Meter-Gateway angeschlossen
        // sind, können An- und Abmeldedatum nur nach dem Eingangsdatum liegen."
        Messtyp::Rlm | Messtyp::Imsys => Some(NbEntscheidung::Reject(RejectReason {
            antwortcode: ERC_VORLAUFFRIST.to_owned(),
            ebd: Some("E_0607".into()),
            detail: format!(
                "Rückwirkende Gas-Abmeldung zum {d} ist bei {}-Messung nicht zulässig — \
                 An- und Abmeldedatum können nur nach dem Eingangsdatum liegen \
                 (GeLi Gas 3.0 Kap. 3.2.1). EBD E_0607 Prüfschritt 50 → A02.",
                anfrage.messtyp
            ),
            check_number: 3,
        })),
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
            ist_erzeugende_marktlokation: false,
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
        assert_eq!(r, NbEntscheidung::Accept);
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

    #[test]
    fn a_repeated_auszug_at_the_same_date_is_a10() {
        let ende = d(2026, Month::April, 1);
        let vs = versorgung(LieferStatus::Unbeliefert, Some(OWN_LF), Some(ende));
        let mut a = anfrage(ende);
        a.transaktionsgrund = Some("E01".to_owned());
        let r = evaluate_abmeldung(&a, Some(&vs), NOW, &cfg());
        assert_eq!(r.antwortcode(), Some("A10"), "{r:?}");
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
        assert_eq!(r, NbEntscheidung::Accept);
    }

    // ── EEG-Marktlokation ─────────────────────────────────────────────────────

    #[test]
    fn an_eeg_abmeldung_must_be_a_monatserster() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::May, 15));
        a.ist_erzeugende_marktlokation = true;
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A02")
        );
    }

    #[test]
    fn an_eeg_abmeldung_needs_a_month_of_lead() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::March, 1));
        a.ist_erzeugende_marktlokation = true;
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A02"),
            "1 March is inside the receipt month"
        );

        let mut ok = anfrage(d(2026, Month::April, 1));
        ok.ist_erzeugende_marktlokation = true;
        assert_eq!(
            evaluate_abmeldung(&ok, Some(&vs), NOW, &cfg()),
            NbEntscheidung::Accept
        );
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
            Some("A02")
        );
    }

    #[test]
    fn a_retroactive_gas_auszug_within_six_weeks_is_accepted_for_slp() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::February, 1));
        a.pid = 44_004;
        a.sparte = Sparte::Gas;
        a.transaktionsgrund = Some("E01".to_owned());
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()),
            NbEntscheidung::Accept
        );
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
            Some("A02")
        );
    }

    /// Gas has no „ein voller Werktag" rule — a next-day Abmeldung is fine.
    #[test]
    fn a_future_gas_abmeldung_needs_no_werktag_gap() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let mut a = anfrage(d(2026, Month::March, 3));
        a.pid = 44_004;
        a.sparte = Sparte::Gas;
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()),
            NbEntscheidung::Accept
        );
    }
}
