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
//! # Two branches, two alphabets
//!
//! Prüfschritt 10 („verbrauchende oder ruhende Marktlokation?") splits Strom
//! into branches that share **no** Antwortcode — including the Zustimmung. Every
//! question below is asked twice, once per branch, and answering an erzeugende
//! Abmeldung out of the verbrauchend codes states a code that is valid for the
//! tree and wrong for the Anwendungsfall.
//!
//! | Prüfschritt | Question | verbrauchend / ruhend | erzeugend |
//! |---|---|---|---|
//! | — | The MaLo is known to this NB, and the requesting LF holds an assignment | `Escalate` | `Escalate` |
//! | 50 / 500+520 | Vorlauffrist, und bei erzeugenden der Monatserster | `A02` | `A21` (Datum) / `A22` (Frist) |
//! | 90 / 570 | Aufhebung zum bestätigten Zuordnungsbeginn | `A06` | `A23` |
//! | 80 | E/G begann innerhalb 3 Monaten (nur `Z41`) | `A05` | — |
//! | 100–130 / 580–610 | Kein bereits bestätigtes Lieferende zum selben Datum | `A09` / `A10` | `A25` / `A26` |
//! | 140 / 620 | — | Zustimmung `A11` | Zustimmung **`A27`** |
//!
//! Gas has no such split: `G_0007` publishes one code space, answering the
//! Vorlauffrist with `E17`, a settled Lieferende with `Z08` and the Zustimmung
//! with `E15`.
//!
//! # What is not decided here
//!
//! Prüfschritte 10–30 (Kundenanlagen-Herauslösung) turn on whether the
//! Marktlokation is a „ruhende Marktlokation" of a Kundenanlage (§ 20 Abs. 1d
//! EnWG / § 10c EEG), which this projection does not record; they are not
//! evaluated, so `A01` is catalogued and unreachable.
//!
//! Prüfschritt 130 / 610 is the clearest case of a fact that is *absent* rather
//! than unmodelled. It asks whether the **already confirmed** Abmeldung stated a
//! move-out reason — a fact about an earlier message. `A10` / `A26` is only
//! correct when it did; when it did not, the tree continues and *confirms*.
//! Choosing the refusal because the *new* request names an Auszug answers a
//! different question, and does so in the direction that refuses, so it
//! escalates. Escalation is the § 20 EnWG-safe direction: an unfounded
//! Ablehnung keeps a customer bound to a supplier they have left.
//!
//! Prüfschritte 530–550 (Tranche, Direktvermarktungspflicht, zeitgleiche
//! Abmeldung der weiteren Tranchen) carry no code and no branch — every outcome
//! converges on 560 — so there is nothing to evaluate.
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
/// (`E_0607` Prüfschritt 120), which lists exactly two.
///
/// UTILMD MIG Strom S2.2 `SG4 STS+7` DE 9013: `E01` „Ein-/Auszug (Umzug)" and
/// `Z33` „Auszug wegen Stilllegung" — whose own MIG note is „bei allen anderen
/// Auszügen ist E01 zu verwenden", so the pair is closed. **`E02` is not one of
/// them**: it is „Einzug in Neuanlage", an Anmeldegrund, and a customer moving
/// *in* is not evidence that one moved out.
const AUSZUG_GRUENDE: &[&str] = &["E01", AUSZUG_STILLLEGUNG];

/// `Z33` „Auszug wegen Stilllegung" — the single Grund `E_0607` Prüfschritt 600
/// asks for, where the verbrauchend branch's 120 accepts either of
/// [`AUSZUG_GRUENDE`].
///
/// Taken from the crate root rather than spelled again — `E_0609` Prüfschritt 50
/// asks the same question on the LF side, and one wire code needs one spelling.
use crate::STILLLEGUNG as AUSZUG_STILLLEGUNG;

/// `Z41` „Ende der ESV ohne Folgelieferung" — the Grund `E_0607` Prüfschritt 70
/// routes on.
const ESV_ENDE_OHNE_FOLGE: &str = "Z41";

/// How far back Prüfschritt 80 looks for the Beginn der Ersatz-/Grundversorgung
/// — the same three months § 38 Abs. 4 EnWG caps the Ersatzversorgung at.
const ESV_RUECKBLICK_MONATE: u32 = 3;

/// `ZH2` „Aufhebung einer zukünftigen Zuordnung wegen aufgehobenem
/// Vertragsverhältnis" — the Grund `E_0607` Prüfschritte 60 / 560 route on.
const AUFHEBUNG_VERTRAGSVERHAELTNIS: &str = "ZH2";

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
    // „Der" Lieferant is a list: a tranchierte Marktlokation has several, and
    // the question is whether the requesting one is among them — abmelden is
    // per assignment, not per Marktlokation.
    //
    // A settled Lieferende at this very date passes too, and must: Prüfschritt
    // 100 / 580 („wurde die Zuordnung … bereits durch eine Bestätigung
    // beendet?") is reached precisely when the LF is *no longer* assigned, so
    // demanding a running one here would put `A09` / `A10` / `A25` / `A26` out
    // of reach.
    let bereits_beendet =
        vs.lieferstatus == LieferStatus::Unbeliefert && vs.lieferende == Some(anfrage.abmeldedatum);
    if !bereits_beendet && !vs.aktive().any(|z| z.lf_mp_id == anfrage.lf_mp_id) {
        let zugeordnet: Vec<&str> = vs.aktive().map(|z| z.lf_mp_id.as_str()).collect();
        return NbEntscheidung::Escalate {
            reason: if zugeordnet.is_empty() {
                format!(
                    "MaLo {} carries no assigned Lieferant (lieferstatus = {}), so the \
                     Abmeldung by {} cannot be matched against one.",
                    anfrage.malo_id, vs.lieferstatus, anfrage.lf_mp_id
                )
            } else {
                format!(
                    "MaLo {} is assigned to LF {}, but the Abmeldung came from {}. \
                     {} Prüfschritt 110 — operator review required.",
                    anfrage.malo_id,
                    zugeordnet.join(", "),
                    anfrage.lf_mp_id,
                    tree(sparte)
                )
            },
        };
    }

    let today = mako_fristen::berlin_date(now);

    // ── Prüfschritt 10: verbrauchende/ruhende oder erzeugende Marktlokation? ──
    //
    // „nein → 500" opens a second branch that shares **no** Antwortcode with
    // the first — including its Zustimmung. Gas has no such split: `G_0007`
    // publishes one code space.
    if sparte == Sparte::Strom && !anfrage.marktlokationsart.ist_verbrauchend_oder_ruhend() {
        return e_0607_erzeugend(anfrage, vs, today, *config);
    }
    e_0607_verbrauchend(anfrage, vs, sparte, today, *config)
}

/// Prüfschritt 90 / 570 — „Erfolgt die Aufhebung einer zukünftigen Zuordnung zu
/// demselben Zeitpunkt, welcher dem Lieferanten im Lieferbeginn bestätigt
/// wurde?"
///
/// One question asked in both branches with different codes (`A06` resp.
/// `A23`). The Zeitpunkt „welcher dem Lieferanten im Lieferbeginn bestätigt
/// wurde" is the Zuordnungsbeginn of this LF's assignment, which the projection
/// carries. `None` = the dates agree, or the Grund is not an Aufhebung.
fn check_aufhebung_zeitpunkt(
    anfrage: &AbmeldungAnfrage,
    vs: &VersorgungsStatusRecord,
    code_: &'static str,
    pruefschritt: u16,
) -> Option<NbEntscheidung> {
    if anfrage.transaktionsgrund.as_deref() != Some(AUFHEBUNG_VERTRAGSVERHAELTNIS) {
        return None;
    }
    let bestaetigt = vs
        .zuordnungen
        .iter()
        .find(|z| z.lf_mp_id == anfrage.lf_mp_id)
        .and_then(|z| z.zuordnungsbeginn);
    match bestaetigt {
        Some(beginn) if beginn == anfrage.abmeldedatum => None,
        Some(beginn) => Some(NbEntscheidung::Reject(RejectReason::new(
            EBD_ABMELDUNG_NB,
            code(Sparte::Strom, code_),
            pruefschritt,
            format!(
                "Die Aufhebung einer zukünftigen Zuordnung nennt den {}, im Lieferbeginn \
                 bestätigt wurde aber der {beginn}. Beide müssen denselben Zeitpunkt angeben.",
                anfrage.abmeldedatum
            ),
        ))),
        // No assignment to lift. Refusing would state that the dates disagree,
        // which is not what was found.
        None => Some(NbEntscheidung::Escalate {
            reason: format!(
                "MaLo {}: the Abmeldung lifts a zukünftige Zuordnung, but no Zuordnung of LF \
                 {} is recorded to compare its Zeitpunkt against. E_0607 Prüfschritt \
                 {pruefschritt} — operator review required.",
                anfrage.malo_id, anfrage.lf_mp_id
            ),
        }),
    }
}

/// `E_0607` Prüfschritte 50–140 — verbrauchende und ruhende Marktlokation, and
/// the whole of the Gas tree `G_0007`.
fn e_0607_verbrauchend(
    anfrage: &AbmeldungAnfrage,
    vs: &VersorgungsStatusRecord,
    sparte: Sparte,
    today: Date,
    config: NetzCheckConfig,
) -> NbEntscheidung {
    // ── Prüfschritt 50: Vorlauffrist ─────────────────────────────────────────
    if let Some(violation) = check_vorlauffrist(anfrage, today, config) {
        return violation;
    }

    // ── Prüfschritte 60/90: Aufhebung einer zukünftigen Zuordnung ────────────
    //
    // Strom only: `G_0007` publishes no counterpart, and putting `A06` on a
    // 44006 would state a code the Gas Codeliste does not contain.
    if sparte == Sparte::Strom
        && let Some(refusal) = check_aufhebung_zeitpunkt(anfrage, vs, "A06", 90)
    {
        return refusal;
    }

    // ── Prüfschritte 70/80: Ende der ESV ohne Folgelieferung ─────────────────
    //
    // 80 asks whether an Ersatz-/Grundversorgung *began* within three months of
    // the Endezeitpunkt this Abmeldung names — „eine Lieferende mit dem Grund
    // ‚Ende der ESV ohne Folgelieferung' kann nur in dem Fall vorliegen, wenn
    // diese Marktlokation innerhalb der letzten 3 Monate auch über den Use-Case
    // ‚Beginn der Ersatz-/Grundversorgung' vom NB beim LF angemeldet wurde."
    // `eog_seit` is exactly that Lieferbeginn.
    if sparte == Sparte::Strom && anfrage.transaktionsgrund.as_deref() == Some(ESV_ENDE_OHNE_FOLGE)
    {
        let fenster_beginn = months_before(anfrage.abmeldedatum, ESV_RUECKBLICK_MONATE);
        let innerhalb = vs
            .eog_seit
            .is_some_and(|seit| seit >= fenster_beginn && seit <= anfrage.abmeldedatum);
        if !innerhalb {
            return NbEntscheidung::Reject(RejectReason::new(
                EBD_ABMELDUNG_NB,
                code(Sparte::Strom, "A05"),
                80,
                format!(
                    "Die Marktlokation wurde nicht innerhalb der letzten 3 Monate zur \
                     Ersatz-/Grundversorgung angemeldet (eog_seit = {:?}, Fenster ab \
                     {fenster_beginn}) — somit kann es sich nicht um eine Beendigung einer \
                     ESV handeln.",
                    vs.eog_seit
                ),
            ));
        }
    }

    // ── Prüfschritte 100–130: kein bereits bestätigtes Lieferende ────────────
    //
    // `lieferende` is only set once the NB has confirmed one, so its presence
    // *is* the „bereits bestätigt" condition (100), and `lieferstatus`
    // distinguishes a settled end from a running supply — which is Prüfschritt
    // 110 „Ist der anfragende LF am Folgetag des Abmeldungsdatum noch
    // zugeordnet?", whose „ja" goes straight to 140 and confirms.
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

/// `E_0607` Prüfschritte 500–620 — erzeugende Marktlokation und Tranche.
///
/// The same questions as the branch above, asked again with **different codes**
/// and, in two places, a different shape: Prüfschritt 590 „ist der LF am
/// Folgetag noch zugeordnet?" goes to 600 on „ja" where the verbrauchend
/// branch's 110 goes straight to the Bestätigung, and the Zustimmung is `A27`,
/// not `A11`.
///
/// Prüfschritte 530–550 (Tranche, Direktvermarktungspflicht, zeitgleiche
/// Abmeldung der weiteren Tranchen) carry **no code and no branch** — every
/// outcome converges on 560 — so they are a note in the published tree rather
/// than a decision, and there is nothing here to evaluate.
fn e_0607_erzeugend(
    anfrage: &AbmeldungAnfrage,
    vs: &VersorgungsStatusRecord,
    today: Date,
    config: NetzCheckConfig,
) -> NbEntscheidung {
    let strom = |c: &'static str, schritt: u16, detail: String| {
        NbEntscheidung::Reject(RejectReason::new(
            EBD_ABMELDUNG_NB,
            code(Sparte::Strom, c),
            schritt,
            detail,
        ))
    };
    let d = anfrage.abmeldedatum;

    // ── 500: Lieferende auf dem 1. eines Kalendermonats? ─────────────────────
    if d.day() != 1 {
        return strom(
            "A21",
            500,
            format!(
                "Abmeldung einer erzeugenden Marktlokation zum {d}, das kein Monatserster ist. \
                 Das Lieferende muss auf dem 1. eines Kalendermonats 00:00 Uhr liegen \
                 (§ 21b Abs. 1 EEG 2023; GPKE Teil 2 § 2.5.1)."
            ),
        );
    }

    // ── 520: mindestens einen Monat vor dem Zuordnungsende? ──────────────────
    let latest_ut = months_before(d, config.eeg_zuordnung_vorlauf_monate);
    if today > latest_ut {
        return strom(
            "A22",
            520,
            format!(
                "Vorlauffrist nicht eingehalten: Abmeldung einer erzeugenden Marktlokation zum \
                 {d}, spätester ÜT {latest_ut} (ein Monat vor dem Zuordnungsende; Eingang \
                 {today})."
            ),
        );
    }

    // ── 560/570: Aufhebung einer zukünftigen Zuordnung ───────────────────────
    if let Some(refusal) = check_aufhebung_zeitpunkt(anfrage, vs, "A23", 570) {
        return refusal;
    }

    // ── 580–610: kein bereits bestätigtes Lieferende zum selben Datum ────────
    if vs.lieferstatus == LieferStatus::Unbeliefert && vs.lieferende == Some(d) {
        // 590 „ja" → 600 asks for „Auszug wegen Stilllegung" specifically; the
        // verbrauchend branch's 120 accepts either Auszugsgrund. `E02` is the
        // one DE 9013 code that carries it.
        if anfrage.transaktionsgrund.as_deref() != Some(AUSZUG_STILLLEGUNG) {
            return strom(
                "A25",
                600,
                format!(
                    "Lieferende zum Abmeldedatum {d} wurde bereits bestätigt, und die neue \
                     Abmeldung nennt nicht „Auszug wegen Stilllegung“ ({:?}).",
                    anfrage.transaktionsgrund
                ),
            );
        }
        // 610 asks about the *already confirmed* Abmeldung's Transaktionsgrund,
        // which the projection does not keep — `A26` and „continue to 620 and
        // confirm" are both live, and one of them refuses.
        return NbEntscheidung::Escalate {
            reason: format!(
                "MaLo {}: a Lieferende zum {d} is already confirmed and the new Abmeldung \
                 names „Auszug wegen Stilllegung“. E_0607 Prüfschritt 610 decides between \
                 A26 and a Bestätigung on the *earlier* message's Transaktionsgrund, which \
                 is not in the supply projection — operator review required.",
                anfrage.malo_id
            ),
        };
    }

    // ── 620: Zustimmung ──────────────────────────────────────────────────────
    NbEntscheidung::accept(EBD_ABMELDUNG_NB, code(Sparte::Strom, "A27"))
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
    use mako_markt::repository::{LfZuordnung, ZuordnungsStatus};
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
            zuordnungen: lf_mp_id
                .map(|lf| vec![LfZuordnung::ganz(lf, ZuordnungsStatus::Aktiv)])
                .unwrap_or_default(),
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

    /// Prüfschritt 100 is reached exactly when the LF is **no longer** assigned
    /// — that is what „wurde die Zuordnung … bereits durch eine Bestätigung
    /// beendet?" means. A precondition demanding a running assignment therefore
    /// escalates every case the step exists to answer, and `A09` / `A10` /
    /// `A25` / `A26` become unreachable.
    ///
    /// The projection state here is the one `end_supply` actually produces:
    /// `Unbeliefert`, no assignment left, `lieferende` recorded.
    #[test]
    fn a_settled_lieferende_is_reachable_without_a_running_assignment() {
        let vs = versorgung(
            LieferStatus::Unbeliefert,
            None, // end_supply removed it — this is the real shape
            Some(d(2026, Month::May, 1)),
        );
        let mut a = anfrage(d(2026, Month::May, 1));
        a.transaktionsgrund = Some("ZT4".to_owned()); // kein Auszugsgrund → 120 „nein"
        assert_eq!(
            evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A09"),
            "the step that exists for an ended assignment must not need a running one"
        );
    }

    /// Prüfschritt 80 — „Ende der ESV ohne Folgelieferung" (`Z41`) is only
    /// admissible when an Ersatz-/Grundversorgung began within three months of
    /// the Endezeitpunkt. `eog_seit` is that Lieferbeginn.
    #[test]
    fn an_esv_ende_needs_an_eog_within_three_months() {
        let ende = d(2026, Month::May, 1);
        let mut a = anfrage(ende);
        a.transaktionsgrund = Some("Z41".to_owned());

        // Never in Ersatz-/Grundversorgung at all.
        let keine = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        assert_eq!(
            evaluate_abmeldung(&a, Some(&keine), NOW, &cfg()).antwortcode(),
            Some("A05")
        );

        // Began too long ago.
        let mut alt = versorgung(LieferStatus::Ersatzversorgung, Some(OWN_LF), None);
        alt.eog_seit = Some(d(2025, Month::December, 1));
        assert_eq!(
            evaluate_abmeldung(&a, Some(&alt), NOW, &cfg()).antwortcode(),
            Some("A05")
        );

        // Inside the window — 80 „ja" → 100, and the tree confirms.
        let mut frisch = versorgung(LieferStatus::Ersatzversorgung, Some(OWN_LF), None);
        frisch.eog_seit = Some(d(2026, Month::March, 1));
        assert_eq!(
            evaluate_abmeldung(&a, Some(&frisch), NOW, &cfg()).antwortcode(),
            Some("A11")
        );
    }

    /// `Z33` „Auszug wegen Stilllegung" is the second Auszugsgrund Prüfschritt
    /// 120 lists; `E02` is „Einzug in Neuanlage" and is not a move-out at all.
    #[test]
    fn the_auszug_gruende_are_e01_and_z33() {
        let vs = versorgung(
            LieferStatus::Unbeliefert,
            None,
            Some(d(2026, Month::May, 1)),
        );
        for grund in ["E01", "Z33"] {
            let mut a = anfrage(d(2026, Month::May, 1));
            a.transaktionsgrund = Some(grund.to_owned());
            assert!(
                evaluate_abmeldung(&a, Some(&vs), NOW, &cfg()).is_escalate(),
                "{grund} reaches 130, which turns on the earlier message's Grund"
            );
        }
        // „Einzug in Neuanlage" is an Anmeldegrund: 120 answers „nein" → A09.
        let mut einzug = anfrage(d(2026, Month::May, 1));
        einzug.transaktionsgrund = Some("E02".to_owned());
        assert_eq!(
            evaluate_abmeldung(&einzug, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A09")
        );
    }

    // ── Erzeugende Marktlokation (Prüfschritte 500–620) ──────────────────────

    fn erzeugend(abmeldedatum: Date) -> AbmeldungAnfrage {
        let mut a = anfrage(abmeldedatum);
        a.marktlokationsart = Marktlokationsart::Erzeugend;
        a
    }

    /// Prüfschritt 10 „nein" leaves the verbrauchend branch for one that shares
    /// **no** Antwortcode with it. Answering an erzeugende Abmeldung out of the
    /// verbrauchend codes states a code that is valid for the tree and wrong
    /// for the Anwendungsfall — including on the happy path, where the
    /// Zustimmung is `A27` and not `A11`.
    #[test]
    fn the_erzeugende_branch_answers_from_its_own_alphabet() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        let verbrauchend: &[&str] = &["A01", "A02", "A05", "A06", "A09", "A10", "A11"];
        for a in [
            erzeugend(d(2026, Month::May, 15)),  // 500 — kein Monatserster
            erzeugend(d(2026, Month::March, 1)), // 520 — Vorlauffrist
            erzeugend(d(2026, Month::May, 1)),   // 620 — Zustimmung
        ] {
            let got = evaluate_abmeldung(&a, Some(&vs), NOW, &cfg());
            let code = got.antwortcode().expect("the branch states a code");
            assert!(
                !verbrauchend.contains(&code),
                "{code} belongs to E_0607's verbrauchend branch: {got:?}"
            );
        }
    }

    /// Prüfschritt 500 — „Ist das angegebene Datum ‚Lieferende' der 1. eines
    /// Kalendermonats 00:00 Uhr?" `A21`, whose Bedeutung is that date rule;
    /// `A02` („Vorlauffrist nicht eingehalten") states a different reason.
    #[test]
    fn an_erzeugende_abmeldung_must_be_a_monatserster() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        assert_eq!(
            evaluate_abmeldung(&erzeugend(d(2026, Month::May, 15)), Some(&vs), NOW, &cfg())
                .antwortcode(),
            Some("A21")
        );
    }

    /// Prüfschritt 520 — „Liegt die Abmeldung mindestens einen Monat vor
    /// Zuordnungsende vor?" (§ 21b Abs. 1 EEG 2023).
    #[test]
    fn an_erzeugende_abmeldung_needs_a_month_of_lead() {
        let vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        assert_eq!(
            evaluate_abmeldung(&erzeugend(d(2026, Month::March, 1)), Some(&vs), NOW, &cfg())
                .antwortcode(),
            Some("A22"),
            "1 March is inside the receipt month"
        );

        // 1 April is one day short too: „spätester ÜT liegt 1 Monat vor dem
        // Zuordnungsende" makes 1 March the latest ÜT, and receipt is 2 March.
        assert_eq!(
            evaluate_abmeldung(&erzeugend(d(2026, Month::April, 1)), Some(&vs), NOW, &cfg())
                .antwortcode(),
            Some("A22")
        );

        let ok = evaluate_abmeldung(&erzeugend(d(2026, Month::May, 1)), Some(&vs), NOW, &cfg());
        assert!(ok.is_accept(), "{ok:?}");
        assert_eq!(ok.antwortcode(), Some("A27"), "620, not 140");
    }

    /// Prüfschritt 600 asks for „Auszug wegen Stilllegung" **specifically**,
    /// where the verbrauchend branch's 120 accepts either Auszugsgrund — so
    /// `E01` (Ein-/Auszug) refuses here and defers there.
    #[test]
    fn six_hundred_wants_stilllegung_not_any_auszug() {
        let vs = versorgung(
            LieferStatus::Unbeliefert,
            None,
            Some(d(2026, Month::May, 1)),
        );
        let mut umzug = erzeugend(d(2026, Month::May, 1));
        umzug.transaktionsgrund = Some("E01".to_owned());
        assert_eq!(
            evaluate_abmeldung(&umzug, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A25")
        );

        // `E02` reaches 610, which turns on the *earlier* message's Grund — a
        // fact the projection does not keep, so it escalates rather than guess.
        let mut stilllegung = erzeugend(d(2026, Month::May, 1));
        stilllegung.transaktionsgrund = Some("Z33".to_owned());
        assert!(
            evaluate_abmeldung(&stilllegung, Some(&vs), NOW, &cfg()).is_escalate(),
            "610 decides on a fact this projection does not hold"
        );
    }

    /// Prüfschritt 570 — the Aufhebung must name the same Zeitpunkt the NB
    /// confirmed in the Lieferbeginn.
    #[test]
    fn an_aufhebung_must_match_the_confirmed_zuordnungsbeginn() {
        let mut vs = versorgung(LieferStatus::Beliefert, Some(OWN_LF), None);
        vs.zuordnungen[0].zuordnungsbeginn = Some(d(2026, Month::June, 1));

        let mut wrong = erzeugend(d(2026, Month::May, 1));
        wrong.transaktionsgrund = Some("ZH2".to_owned());
        assert_eq!(
            evaluate_abmeldung(&wrong, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A23")
        );

        let mut right = erzeugend(d(2026, Month::June, 1));
        right.transaktionsgrund = Some("ZH2".to_owned());
        assert_eq!(
            evaluate_abmeldung(&right, Some(&vs), NOW, &cfg()).antwortcode(),
            Some("A27"),
            "matching dates fall through 580 to the Bestätigung"
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
