//! The INVOIC/REMADV walk the MSB's invoices are checked with — **one engine,
//! three families**.
//!
//! The BDEW publishes the same Rechnungsprüfung three times over, under twelve
//! EBD numbers, because a code is resolved against the tree the answer names
//! and the three relationships name different trees:
//!
//! | Familie | Rechnung | erneut | Nicht-Zahlungsavis | Storno | Prüfende Rolle |
//! |---|---|---|---|---|---|
//! | [`ESA`] (WiM T2 Kap. 4.5) | `E_0264` | `E_0266` | `E_0265` | `E_0267` | ESA |
//! | [`PREISBLATT_B_LF`] (AWH Kap. 9.3) | `E_0270` | `E_0276` | `E_0271` | `E_0272` | LF |
//! | [`PREISBLATT_B_NB`] (AWH Kap. 9.4) | `E_0273` | `E_0277` | `E_0274` | `E_0275` | NB |
//!
//! The walk is the same — Kopf 10–100, Position 300–430, Summe 500–550. Three
//! things differ, and they are the whole of [`RechnungsFamilie`]:
//!
//! 1. **The second round's Prüfschritt 1 code** — „Konnte der MSB alle Einwände
//!    entkräften?" is `A25` in `E_0266`, **`AC1`** in `E_0276`/`E_0277`.
//! 2. **Prüfschritte 80 and 90 are Preisblatt-B only** — `A08` (Preisblatt-
//!    Version) and `A25` (doppelter Abrechnungszeitraum). An ESA has no
//!    Preisblatt: its prices come from the accepted QUOTES 15003.
//! 3. **`A90` therefore sits at 90 or 100.**
//!
//! (1) and (2) together are why this is parameterised rather than copied:
//! **`A25` is the ESA's second-round refusal and the Preisblatt-B doppelter
//! Abrechnungszeitraum** — one spelling, two Prüfschritte, two meanings, both
//! riding REMADV 33003.
//!
//! # Alles oder nichts
//!
//! WiM Teil 2 UC 4.5.1 states the „Alles-oder-Nichts-Prinzip": an invoice is
//! accepted in full or refused in full, and there is no Teilzahlung. So a
//! single Befund refuses the whole invoice — which is why
//! [`RechnungsAntwort::ist_zustimmung`] is „no Befund at all" rather than a
//! severity judgement.
//!
//! # The answer shape decides the REMADV Prüfidentifikator
//!
//! REMADV AHB 1.0a § 3.1.2 carries **33003** „Abweisung Kopf und Summe" and
//! **33004** „Abweisung Position", whose `SG7 AJT` DE 1082 admits the Rechnungs-
//! trees. The plain Abweisung **33002** of § 3.1.1 admits the Storno trees
//! instead, which answer with a single code.
//! [`RechnungsAntwort::remadv_pid`] and [`StornoAntwort::remadv_pid`] are that
//! mapping, so an answer cannot be sent under a Prüfidentifikator its tree is
//! not published for.
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2b, WiM Strom Teil 2 Kap. 4.5
//! - BDEW *AWH Prozesse zur Änderung der Technik an Lokationen* V1.1, Kap. 9.3/9.4
//! - *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 8.27 und 9.3/9.4
//! - REMADV AHB 1.0a § 3.1.1 / § 3.1.2, COMDIS AHB 1.0h, INVOIC AHB 1.0b

use time::Date;

use crate::antwort::AntwortDetail;
use crate::codes::{self, lookup};

/// One relationship's four Entscheidungsbäume, plus the three ways the walk
/// differs between families.
///
/// Every field is read off the published trees; none is a policy choice. See
/// the module note for what each one is and why it cannot be inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RechnungsFamilie {
    /// The first-round Rechnungsprüfung — `E_0264` / `E_0270` / `E_0273`.
    pub rechnung: &'static str,
    /// The second look after a COMDIS 29001 — `E_0266` / `E_0276` / `E_0277`.
    pub erneut: &'static str,
    /// The MSB's check of the Nicht-Zahlungsavis — `E_0265` / `E_0271` / `E_0274`.
    pub nicht_zahlungsavis: &'static str,
    /// „Ist eine Antwort auf die Stornierung erforderlich?" — `E_0267` /
    /// `E_0272` / `E_0275`.
    pub storno: &'static str,
    /// The code the **second round's** Prüfschritt 1 publishes for „Konnte der
    /// MSB alle Einwände entkräften? — nein".
    ///
    /// `A25` in `E_0266`, **`AC1`** in `E_0276`/`E_0277`. The question is
    /// identical and the code is not, which is the single most likely place for
    /// a hand-written second copy to be wrong.
    pub erneut_einwand_code: &'static str,
    /// Whether the family publishes the two Preisblatt-B-only Kopf-Prüfschritte:
    /// **80** (`A08`, Preisblatt-Version) and **90** (`A25`, doppelter
    /// Abrechnungszeitraum).
    ///
    /// `false` for the ESA, which has no Preisblatt — its prices come from the
    /// accepted QUOTES 15003 (§ 35 MsbG leaves a Zusatzleistung's Entgelt to be
    /// agreed per request).
    pub hat_preisblatt_pruefung: bool,
}

impl RechnungsFamilie {
    /// The Prüfschritt number of the Kopfebene catch-all `A90`.
    ///
    /// 90 without the Preisblatt-Prüfschritte, 100 with them — the two extra
    /// steps push it down. The number is not decoration: it is what an auditor
    /// holding the EBD matches the Befund against.
    #[must_use]
    pub const fn sonstiger_kopffehler_schritt(&self) -> u16 {
        if self.hat_preisblatt_pruefung {
            100
        } else {
            90
        }
    }
}

/// WiM Strom **Teil 2 Kap. 4.5** — „Abrechnung einer für den ESA erbrachten
/// Leistung". Prüfende Rolle: the **ESA** (except `E_0265`, the MSB's).
pub const ESA: RechnungsFamilie = RechnungsFamilie {
    rechnung: codes::EBD_ESA_RECHNUNG,
    erneut: codes::EBD_ESA_RECHNUNG_ERNEUT,
    nicht_zahlungsavis: codes::EBD_ESA_NICHT_ZAHLUNGSAVIS,
    storno: codes::EBD_ESA_STORNO_RECHNUNG,
    erneut_einwand_code: "A25",
    hat_preisblatt_pruefung: false,
};

/// AWH *Änderung der Technik an Lokationen* **Kap. 9.3** — Abrechnung der
/// Leistungen des Preisblatts B zwischen MSB und **LF**. Prüfende Rolle: the
/// **LF** (except `E_0271`, the MSB's).
pub const PREISBLATT_B_LF: RechnungsFamilie = RechnungsFamilie {
    rechnung: codes::EBD_PREISBLATT_B_RECHNUNG_LF,
    erneut: codes::EBD_PREISBLATT_B_RECHNUNG_ERNEUT_LF,
    nicht_zahlungsavis: codes::EBD_PREISBLATT_B_NICHT_ZAHLUNGSAVIS_LF,
    storno: codes::EBD_PREISBLATT_B_STORNO_LF,
    erneut_einwand_code: "AC1",
    hat_preisblatt_pruefung: true,
};

/// AWH *Änderung der Technik an Lokationen* **Kap. 9.4** — dieselbe Abrechnung
/// zwischen MSB und **NB**. Prüfende Rolle: the **NB** (except `E_0274`).
pub const PREISBLATT_B_NB: RechnungsFamilie = RechnungsFamilie {
    rechnung: codes::EBD_PREISBLATT_B_RECHNUNG_NB,
    erneut: codes::EBD_PREISBLATT_B_RECHNUNG_ERNEUT_NB,
    nicht_zahlungsavis: codes::EBD_PREISBLATT_B_NICHT_ZAHLUNGSAVIS_NB,
    storno: codes::EBD_PREISBLATT_B_STORNO_NB,
    erneut_einwand_code: "AC1",
    hat_preisblatt_pruefung: true,
};

/// Every family this module walks, for exhaustive tests.
pub const FAMILIEN: &[RechnungsFamilie] = &[ESA, PREISBLATT_B_LF, PREISBLATT_B_NB];

/// The family whose trees an inbound INVOIC resolves to.
///
/// A thin bridge from [`crate::codes::rechnungspruefung`]'s `(pid, empfaenger,
/// gegenstand)` triple, so a caller that already resolved the tree names does
/// not resolve them a second time by hand.
#[must_use]
pub fn familie_fuer(
    pid: u32,
    empfaenger: mako_fristen::vorlauf::RechnungEmpfaenger,
    gegenstand: codes::MsbRechnungsgegenstand,
) -> Option<RechnungsFamilie> {
    let trees = codes::rechnungspruefung(pid, empfaenger, gegenstand)?;
    FAMILIEN
        .iter()
        .copied()
        .find(|f| f.rechnung == trees.rechnung)
}

/// Minimum Werktage between the Rechnungseingang and the Zahlungsziel.
///
/// `E_0264` Prüfschritt 70's own Hinweis: „Fälligkeit unterschritten bedeutet:
/// Zahlungsziel ≤ 10 WT zum Rechnungseingangsdatum". WiM Teil 2 UC 4.5.2 Nr. 1
/// states the same rule from the sender's side („Das Zahlungsziel darf 10 WT
/// nach Empfang der Rechnung nicht unterschreiten"), which is why this is a
/// refusal and not a warning: the MSB had the rule too.
pub const ZAHLUNGSZIEL_MINDEST_WT: u32 = 10;

/// Which level of the EBD a Befund came from.
///
/// The three are not decoration: they decide which REMADV Prüfidentifikator
/// carries the answer, and the Positionsebene additionally carries the
/// Positionsnummer the MSB has to correct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "ebene", content = "position")]
pub enum Ebene {
    /// Prüfschritte 10–90. A Kopf-level refusal ends the walk: the EBD's own
    /// rule is „werden keine weiteren Prüfschritte mehr durchgeführt".
    Kopf,
    /// Prüfschritte 300–430, with the `SG26 LIN` Positionsnummer.
    Position(u16),
    /// Prüfschritte 500–550.
    Summe,
}

/// One refusal the tree produced.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Befund {
    /// Kopf, Position or Summe — see [`Ebene`].
    pub ebene: Ebene,
    /// The resolved Antwortcode and the tree that publishes it.
    #[serde(flatten)]
    pub antwort: AntwortDetail,
    /// The published Prüfschritt number (`10`, `320`, `540`), so an auditor
    /// holding the EBD can find the row this came from.
    pub pruefschritt: u16,
    /// Human-readable explanation. When
    /// [`AntwortDetail::braucht_bemerkung`] is set this is also what goes into
    /// the REMADV `SG7 FTX+ABO` / `SG12 FTX+ABO`.
    pub detail: String,
}

/// The result of `E_0264` or `E_0266` — a **set** of Befunde, not one code.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RechnungsAntwort {
    /// `E_0264` (first round) or `E_0266` (after a COMDIS 29001).
    pub tree: &'static str,
    /// Every refusal, in Prüfschritt order. Empty means the Zahlungsavis.
    pub befunde: Vec<Befund>,
}

impl RechnungsAntwort {
    /// `true` when nothing was refused — „Zahlung der Rechnung avisieren und im
    /// Zahlungslauf berücksichtigen" (Prüfschritt 560).
    #[must_use]
    pub fn ist_zustimmung(&self) -> bool {
        self.befunde.is_empty()
    }

    /// Every Antwortcode this answer carries, in Prüfschritt order.
    ///
    /// The REMADV repeats the `SG7`/`SG12 AJT` once per Befund; this is that
    /// list, for a caller rendering it and for tests.
    #[must_use]
    pub fn antwortcodes(&self) -> Vec<&str> {
        self.befunde
            .iter()
            .map(|b| b.antwort.antwortcode.as_str())
            .collect()
    }

    /// The REMADV Prüfidentifikator this answer must ride.
    ///
    /// - **33001** Zahlungsavis when nothing was refused. It carries no `AJT`.
    /// - **33004** „Abweisung Position" when the refusals are position-level:
    ///   Prüfschritt 450 sends every position code with its Positionsnummer and
    ///   ends the walk, so the summen level is never reached.
    /// - **33003** „Abweisung Kopf und Summe" otherwise.
    #[must_use]
    pub fn remadv_pid(&self) -> u32 {
        if self.befunde.is_empty() {
            33_001
        } else if self
            .befunde
            .iter()
            .all(|b| matches!(b.ebene, Ebene::Position(_)))
        {
            33_004
        } else {
            33_003
        }
    }

    /// Every Befund's code, in order — the `AJT` DE 4465 values the REMADV
    /// carries.
    #[must_use]
    pub fn codes(&self) -> Vec<&str> {
        self.befunde
            .iter()
            .map(|b| b.antwort.antwortcode.as_str())
            .collect()
    }
}

/// A Leistungszeitraum, or a single Ausführungsdatum written as `von == bis`.
///
/// INVOIC AHB 1.0b lets a position carry either; `E_0264` Prüfschritte 350 and
/// 360 ask about „der Leistungszeitraum bzw. das Ausführungsdatum", so one type
/// serves both and the walk does not branch on which shape arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Zeitraum {
    /// First day covered.
    pub von: Date,
    /// Last day covered (inclusive).
    pub bis: Date,
}

impl Zeitraum {
    /// A single day — an Ausführungsdatum.
    #[must_use]
    pub const fn tag(datum: Date) -> Self {
        Self {
            von: datum,
            bis: datum,
        }
    }

    /// `true` when the two share at least one day (Prüfschritt 360).
    #[must_use]
    pub const fn ueberschneidet(self, other: Self) -> bool {
        self.von.to_julian_day() <= other.bis.to_julian_day()
            && other.von.to_julian_day() <= self.bis.to_julian_day()
    }

    /// `true` when `self` lies entirely inside `outer` (Prüfschritt 350).
    #[must_use]
    pub const fn liegt_in(self, outer: Self) -> bool {
        self.von.to_julian_day() >= outer.von.to_julian_day()
            && self.bis.to_julian_day() <= outer.bis.to_julian_day()
    }
}

/// One `SG26 LIN` position of the MSB's Rechnung, as the tree asks about it.
///
/// One field per Prüfschritt, deliberately: the correspondence is what lets an
/// auditor holding the EBD read this struct, and it is what keeps a wrong code
/// from being reachable at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionsFakten {
    /// `SG26 LIN` DE 1082 — the Positionsnummer the answer has to name.
    pub positionsnummer: u16,
    /// `SG26 PIA+5` DE 7143 `Z09` — the Artikel-ID, when the position carries
    /// one. `None` is Prüfschritt 300 „nein" by itself: a position that names
    /// no Artikel-ID cannot name the one from the Bestellung.
    pub artikel_id: Option<String>,
    /// Prüfschritt 300 — is this Artikel-ID one the accepted Angebot priced?
    /// Ignored when [`Self::artikel_id`] is `None`.
    pub artikel_id_aus_bestellung: bool,
    /// Prüfschritt 310 — did the MSB actually perform the billed service?
    /// `None` when the ESA holds no record either way, which escalates rather
    /// than refusing a service that may well have been delivered.
    pub leistung_erbracht: Option<bool>,
    /// Prüfschritt 320 — does the price match the Angebot valid for this
    /// position's Ausführungsdatum / Abrechnungszeitraum? `None` when no
    /// accepted Angebot is on record.
    pub preis_wie_angebot: Option<bool>,
    /// Prüfschritt 330 — is the Umsatzsteuersatz the one valid for the period?
    pub steuersatz_korrekt: Option<bool>,
    /// Prüfschritte 350/360 — the position's own Leistungszeitraum bzw.
    /// Ausführungsdatum.
    pub zeitraum: Option<Zeitraum>,
    /// Prüfschritt 370 — the Rechnungsnummer of an earlier, not-cancelled
    /// invoice that already billed this Artikel-ID for the same period. The
    /// code's Hinweis („Rechnungsnummer ist anzugeben") is why this is the
    /// number and not a boolean.
    pub bereits_abgerechnet_in: Option<String>,
    /// Prüfschritt 420 — `menge × einzelpreis` ≠ `gesamtpreis`.
    pub rechenfehler: bool,
    /// Prüfschritt 430 — a position-level defect no earlier Prüfschritt names.
    pub sonstiger_fehler: Option<String>,
}

/// One (Steuersatz, Steuerkategorie) combination of the Summenteil.
///
/// Prüfschritte 510 and 520 run **per combination**, and their Hinweis
/// requires the answer to name „den Steuersatz (aus DE5278) und die
/// Steuerkategorie (aus DE5305) des SG52 TAX" — so both travel with the
/// Befund rather than being summarised away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Steuersatzpruefung {
    /// `SG52 TAX` DE 5278 — the rate, as written on the wire (`"19.00"`).
    pub steuersatz: String,
    /// `SG52 TAX` DE 5305 — the Steuerkategorie (`"S"`, `"Z"`, `"E"`).
    pub steuerkategorie: String,
    /// Prüfschritt 510 — does the stated Besteuerungsgrundlage equal the sum of
    /// the positions carrying this rate?
    pub besteuerungsgrundlage_stimmt: bool,
    /// Prüfschritt 520 — does the stated Steuerbetrag equal that sum × rate?
    pub steuerbetrag_stimmt: bool,
}

/// Everything `E_0264` / `E_0266` asks about one inbound INVOIC 31009.
///
/// One field per Prüfschritt. That correspondence is what lets an auditor
/// holding the published tree read this struct, and what keeps a wrong code
/// from being reachable — collapsing the flags into a bitset or a status word
/// would lose both.
#[allow(clippy::struct_excessive_bools)] // one field per Prüfschritt, by design
#[derive(Debug, Clone, PartialEq)]
pub struct RechnungsFakten {
    /// **`E_0266` Prüfschritt 1 only** — could the MSB's COMDIS 29001 rebut
    /// every objection the ESA raised? `None` on the first round, where the
    /// Prüfschritt does not exist. [`pruefe_rechnung`] ignores it;
    /// [`pruefe_rechnung_erneut`] refuses with `A25` on `Some(false)`.
    pub einwaende_entkraeftet: Option<bool>,
    /// Prüfschritt 10 — does the invoice carry the § 14 Abs. 4 UStG content?
    /// `None` escalates rather than refusing a formally correct invoice on a
    /// check nothing ran.
    pub ustg_konform: Option<bool>,
    /// `DTM+137` — the Rechnungsdatum (Prüfschritte 20, 30).
    pub rechnungsdatum: Date,
    /// The day the invoice arrived — the **ÜT of the AS4-Zustellquittung**,
    /// not the local ingest timestamp (Prüfschritte 20, 70).
    pub eingangsdatum: Date,
    /// The head-level Leistungszeitraum bzw. Ausführungsdatum
    /// (Prüfschritte 30, 350).
    pub leistungszeitraum: Option<Zeitraum>,
    /// Prüfschritt 40 — does the invoice reference an ORDERS 17007 this ESA
    /// actually placed? WiM Teil 2 UC 4.5.1: „Eine Rechnung referenziert auf
    /// die zugrundeliegende Bestellung."
    pub bestellung_bekannt: bool,
    /// Prüfschritt 50 — has this Rechnungsnummer been seen from this MSB before?
    pub rechnungsnummer_bereits_verwendet: bool,
    /// Prüfschritt 60 — the fällige Betrag, as a sign. `false` means it is
    /// negative: „Bei der Abrechnung des MSB kann es nicht zu einer
    /// Rückerstattung kommen."
    pub faelliger_betrag_nicht_negativ: bool,
    /// `SG8 DTM+265` — the Zahlungsziel (Prüfschritt 70). `None` skips the
    /// check; an invoice with no Zahlungsziel is a § 14 UStG defect, which is
    /// Prüfschritt 10's business.
    pub zahlungsziel: Option<Date>,
    /// Prüfschritt 80 — **Preisblatt-B families only.** Does the recipient
    /// hold the Preisblatt version the invoice bills against? The sheet is
    /// „Preisblatt Technik" in the PRICAT 27002.
    ///
    /// `None` is „not assessed" and does not refuse. Ignored entirely by the
    /// [`ESA`] family, whose trees publish no such Prüfschritt.
    pub preisblatt_version_gueltig: Option<bool>,
    /// Prüfschritt 90 — **Preisblatt-B families only.** The Rechnungsnummer of
    /// an earlier accepted, not-cancelled invoice that already settled this
    /// Abrechnungszeitraum. `Some` refuses with `A25`, and the code's Hinweis
    /// makes naming the number part of the answer.
    ///
    /// Ignored by the [`ESA`] family — there `A25` is the second round's
    /// Einwand code instead.
    pub zeitraum_bereits_abgerechnet_in: Option<String>,
    /// Prüfschritt 90 resp. 100 — a head-level defect no earlier Prüfschritt
    /// names.
    pub sonstiger_kopffehler: Option<String>,
    /// The `SG26 LIN` positions, in wire order.
    pub positionen: Vec<PositionsFakten>,
    /// Prüfschritt 500 — Artikel-IDs the accepted Angebot priced that this
    /// invoice does not bill. The Hinweis requires them to be named, which is
    /// why this is the list and not a count.
    pub fehlende_artikel_ids: Vec<String>,
    /// Prüfschritte 510/520, one entry per (Steuersatz, Steuerkategorie).
    pub steuersaetze: Vec<Steuersatzpruefung>,
    /// Prüfschritt 540 — does the Rechnungsbetrag equal Σ Besteuerungsgrundlage
    /// + Σ Steuerbetrag?
    pub rechnungsbetrag_stimmt: bool,
    /// Prüfschritt 550 — a Summen-level defect no earlier Prüfschritt names.
    pub sonstiger_summenfehler: Option<String>,
}

/// Walk the family's **first-round** Rechnungsprüfung — `E_0264` / `E_0270` /
/// `E_0273`.
///
/// # Panics
///
/// Only if the family's Codeliste is missing a code this walk names, which the
/// `every_family_publishes_every_code_the_walk_names` test rules out for every
/// family in [`FAMILIEN`].
#[must_use]
pub fn pruefe_rechnung(
    familie: RechnungsFamilie,
    r: &RechnungsFakten,
    cal: crate::HolidayCalendar,
) -> RechnungsAntwort {
    walk(familie, familie.rechnung, r, cal, false)
}

/// Walk the family's **second look**, after the MSB's COMDIS 29001 claimed the
/// invoice was correct — `E_0266` / `E_0276` / `E_0277`.
///
/// Identical to [`pruefe_rechnung`] except for Prüfschritt 1: if the COMDIS did
/// not rebut every objection, the answer is
/// [`RechnungsFamilie::erneut_einwand_code`] and the walk stops. That is a
/// Kopf-level refusal, so it rides REMADV 33003.
///
/// # Panics
///
/// As [`pruefe_rechnung`].
#[must_use]
pub fn pruefe_rechnung_erneut(
    familie: RechnungsFamilie,
    r: &RechnungsFakten,
    cal: crate::HolidayCalendar,
) -> RechnungsAntwort {
    walk(familie, familie.erneut, r, cal, true)
}

fn befund(
    tree: &'static str,
    ebene: Ebene,
    code: &str,
    pruefschritt: u16,
    detail: impl Into<String>,
) -> Befund {
    let resolved = lookup(tree, code).expect("code is published by this tree");
    Befund {
        ebene,
        antwort: AntwortDetail::new(tree, resolved),
        pruefschritt,
        detail: detail.into(),
    }
}

fn walk(
    familie: RechnungsFamilie,
    tree: &'static str,
    r: &RechnungsFakten,
    cal: crate::HolidayCalendar,
    zweite_runde: bool,
) -> RechnungsAntwort {
    let mut befunde = Vec::new();

    // ── Kopfebene ────────────────────────────────────────────────────────────
    //
    // „Führt eine Prüfung zu einer Ablehnung, werden keine weiteren
    // Prüfschritte mehr durchgeführt und ein Antwortcode wird als Ergebnis an
    // den MSB übermittelt." So the Kopfebene returns at most one code, and
    // reaching the positions at all means the head was clean.
    if let Some(kopf) = kopfebene(familie, tree, r, cal, zweite_runde) {
        return RechnungsAntwort {
            tree,
            befunde: vec![kopf],
        };
    }

    // ── Positionsebene ───────────────────────────────────────────────────────
    //
    // „Führt eine Prüfung zu einer Ablehnung, werden auch die weiteren
    // Prüfschritte für diese Position durchlaufen" — every position is walked
    // to the end, and every position is walked. Prüfschritt 450 then ends the
    // EBD if anything was found, so the Summenebene is only reached on clean
    // positions.
    for (i, p) in r.positionen.iter().enumerate() {
        befunde.extend(positionsebene(tree, r, p, i));
    }
    if !befunde.is_empty() {
        return RechnungsAntwort { tree, befunde };
    }

    // ── Summenebene ──────────────────────────────────────────────────────────
    //
    // Prüfschritt 500 ends the EBD on its own („ja → Ende"); 510–550 are
    // collected together.
    if !r.fehlende_artikel_ids.is_empty() {
        return RechnungsAntwort {
            tree,
            befunde: vec![befund(
                tree,
                Ebene::Summe,
                "A21",
                500,
                format!(
                    "Über das bestätigte Angebot vereinbarte Positionen fehlen in der RechnungsFakten: {}",
                    r.fehlende_artikel_ids.join(", ")
                ),
            )],
        };
    }
    befunde.extend(summenebene(tree, r));

    RechnungsAntwort { tree, befunde }
}

// Ten Prüfschritte in sequence, two of them family-gated. Splitting them across
// functions would break the one-to-one correspondence with the published tree,
// which is the property that makes this walk auditable.
#[allow(clippy::too_many_lines)]
fn kopfebene(
    familie: RechnungsFamilie,
    tree: &'static str,
    r: &RechnungsFakten,
    cal: crate::HolidayCalendar,
    zweite_runde: bool,
) -> Option<Befund> {
    let kopf = Ebene::Kopf;

    // 1 — second round only: did the MSB's COMDIS rebut the objections? The
    // code is the family's, not a constant: `A25` in `E_0266` and `AC1` in
    // `E_0276`/`E_0277` for the identical question.
    if zweite_runde && r.einwaende_entkraeftet == Some(false) {
        return Some(befund(
            tree,
            kopf,
            familie.erneut_einwand_code,
            1,
            "Der MSB konnte nicht alle Einwände des Rechnungsempfängers entkräften",
        ));
    }

    // 10 — § 14 Abs. 4 UStG. `None` is „not assessed", which is not „nicht
    // erfüllt": refusing an invoice because nothing checked it would be a
    // rejection mako cannot substantiate.
    if r.ustg_konform == Some(false) {
        return Some(befund(
            tree,
            kopf,
            "A01",
            10,
            "Die RechnungsFakten erfüllt die Anforderungen des § 14 Abs. 4 UStG nicht",
        ));
    }

    // 20 — Rechnungsdatum ≤ Eingangsdatum.
    if r.rechnungsdatum > r.eingangsdatum {
        return Some(befund(
            tree,
            kopf,
            "A02",
            20,
            format!(
                "Rechnungsdatum {} liegt nach dem Eingangsdatum {}",
                r.rechnungsdatum, r.eingangsdatum
            ),
        ));
    }

    // 30 — Rechnungsdatum < Leistungsbeginn is a refusal: the MSB billed a
    // service before performing it.
    if let Some(z) = r.leistungszeitraum
        && r.rechnungsdatum < z.von
    {
        return Some(befund(
            tree,
            kopf,
            "A03",
            30,
            format!(
                "Rechnungsdatum {} liegt vor dem Ausführungsdatum/Leistungszeitraum ab {}",
                r.rechnungsdatum, z.von
            ),
        ));
    }

    // 40 — WiM Teil 2 UC 4.5.1: „Eine Rechnung referenziert auf die
    // zugrundeliegende Bestellung."
    if !r.bestellung_bekannt {
        return Some(befund(
            tree,
            kopf,
            "A04",
            40,
            "Die RechnungsFakten nennt keine Bestellung, die dieser ESA beim MSB platziert hat",
        ));
    }

    // 50 — Rechnungsnummer already used by this Rechnungssteller.
    if r.rechnungsnummer_bereits_verwendet {
        return Some(befund(
            tree,
            kopf,
            "A05",
            50,
            "Die Rechnungsnummer liegt von diesem Rechnungssteller bereits vor",
        ));
    }

    // 60 — no Rückerstattung in an MSB Abrechnung. A credit belongs on a
    // Stornorechnung, which is `E_0267`'s business.
    if !r.faelliger_betrag_nicht_negativ {
        return Some(befund(
            tree,
            kopf,
            "A06",
            60,
            "Der fällige Betrag ist negativ — bei der Abrechnung des MSB kann es nicht zu einer \
             Rückerstattung kommen",
        ));
    }

    // 70 — Zahlungsziel ≤ 10 WT nach dem Rechnungseingangsdatum.
    if let Some(ziel) = r.zahlungsziel {
        let frueheste = mako_fristen::add_werktage(r.eingangsdatum, ZAHLUNGSZIEL_MINDEST_WT, cal);
        if ziel < frueheste {
            return Some(befund(
                tree,
                kopf,
                "A07",
                70,
                format!(
                    "Zahlungsziel {ziel} unterschreitet die {ZAHLUNGSZIEL_MINDEST_WT} Werktage \
                     nach dem Rechnungseingang {} — frühestens {frueheste}",
                    r.eingangsdatum
                ),
            ));
        }
    }

    // 80/90 — Preisblatt-B families only. An ESA has no Preisblatt, so its
    // trees publish neither step and `A25` there means the second round's
    // Einwand instead.
    if familie.hat_preisblatt_pruefung {
        // 80 — „Liegt die angegebene Version des Preisblatts, auf welche sich
        // die Rechnung der bestätigten Bestellung bezieht, vor?" The sheet is
        // called „Preisblatt Technik" in the PRICAT 27002.
        if r.preisblatt_version_gueltig == Some(false) {
            return Some(befund(
                tree,
                kopf,
                "A08",
                80,
                "Die in der RechnungsFakten angegebene Version des Preisblatts liegt nicht vor",
            ));
        }
        // 90 — the Abrechnungszeitraum already settled by an accepted, not
        // cancelled invoice. The Hinweis makes naming that Rechnungsnummer part
        // of the answer.
        if let Some(nummer) = r.zeitraum_bereits_abgerechnet_in.as_deref() {
            return Some(befund(
                tree,
                kopf,
                "A25",
                90,
                format!(
                    "Der Abrechnungszeitraum wurde bereits mit der akzeptierten, nicht \
                     stornierten RechnungsFakten {nummer} abgerechnet"
                ),
            ));
        }
    }

    // 90 resp. 100 — the head-level catch-all. Its Nutzungsmöglichkeit ends
    // 01.04.2027 (ESA) resp. 01.10.2027 (Preisblatt B).
    r.sonstiger_kopffehler.as_ref().map(|detail| {
        befund(
            tree,
            kopf,
            "A90",
            familie.sonstiger_kopffehler_schritt(),
            detail.clone(),
        )
    })
}

// Eight Prüfschritte in sequence. Splitting them across functions would break
// the one-to-one correspondence with the published tree, which is the property
// that makes this walk auditable.
#[allow(clippy::too_many_lines)]
fn positionsebene(
    tree: &'static str,
    r: &RechnungsFakten,
    p: &PositionsFakten,
    index: usize,
) -> Vec<Befund> {
    let mut out = Vec::new();
    let ebene = Ebene::Position(p.positionsnummer);

    // 300 — Artikel-ID aus der Bestellung. „nein → 440": the rest of *this*
    // position is skipped, because without the Artikel-ID there is no Angebot
    // position to compare the remaining Prüfschritte against.
    let artikel_id_ok = p
        .artikel_id
        .as_ref()
        .is_some_and(|_| p.artikel_id_aus_bestellung);
    if !artikel_id_ok {
        out.push(befund(
            tree,
            ebene,
            "A09",
            300,
            match p.artikel_id.as_deref() {
                Some(id) => format!(
                    "Artikel-ID {id} stammt nicht aus der Bestellung, gegen die diese \
                     Übermittlung von Werten beauftragt wurde"
                ),
                None => "Die PositionsFakten nennt keine Artikel-ID aus der Bestellung".to_owned(),
            },
        ));
        return out;
    }

    // 310 — was the billed service performed? `None` is „unbekannt", which is
    // not „nicht erbracht".
    if p.leistung_erbracht == Some(false) {
        out.push(befund(
            tree,
            ebene,
            "A10",
            310,
            "Die abzurechnende Leistung wurde nicht erfolgreich vom MSB durchgeführt",
        ));
    }

    // 320 — the price against the accepted Angebot. An ESA has no Preisblatt;
    // the QUOTES 15003 it ordered against is the whole price basis
    // (§ 35 MsbG leaves the Entgelt einer Zusatzleistung to be agreed per
    // request), so `None` here means mako holds no offer and the position is
    // not comparable rather than wrong.
    if p.preis_wie_angebot == Some(false) {
        out.push(befund(
            tree,
            ebene,
            "A11",
            320,
            "Der Preis der PositionsFakten entspricht nicht dem Preis aus dem Angebot, das zum \
             Ausführungsdatum / Abrechnungszeitraum gültig ist",
        ));
    }

    // 330 — Umsatzsteuersatz for the period.
    if p.steuersatz_korrekt == Some(false) {
        out.push(befund(
            tree,
            ebene,
            "A12",
            330,
            "Für die PositionsFakten ist nicht der für diesen Zeitraum gültige \
             Umsatzsteuersatz angegeben",
        ));
    }

    // 350 — the position's period inside the head's.
    if let (Some(pos), Some(kopf)) = (p.zeitraum, r.leistungszeitraum)
        && !pos.liegt_in(kopf)
    {
        out.push(befund(
            tree,
            ebene,
            "A13",
            350,
            format!(
                "Leistungszeitraum der Position ({} – {}) liegt nicht im Leistungszeitraum des \
                 Kopfteils ({} – {})",
                pos.von, pos.bis, kopf.von, kopf.bis
            ),
        ));
    }

    // 360 — a second position in *this* invoice with the same Artikel-ID and an
    // identical or overlapping period. Computed here rather than asked of the
    // caller: it is a property of the position list, and the tree owns it.
    if let (Some(id), Some(pos)) = (p.artikel_id.as_deref(), p.zeitraum)
        && r.positionen.iter().enumerate().any(|(j, other)| {
            j != index
                && other.artikel_id.as_deref() == Some(id)
                && other.zeitraum.is_some_and(|z| z.ueberschneidet(pos))
        })
    {
        out.push(befund(
            tree,
            ebene,
            "A14",
            360,
            format!(
                "Artikel-ID {id} kommt in dieser RechnungsFakten mehrfach mit identischem oder \
                 überschneidendem Leistungszeitraum vor"
            ),
        ));
    }

    // 370 — already billed on an earlier, not-cancelled invoice. The code's
    // Hinweis makes naming that Rechnungsnummer part of the answer.
    if let Some(nummer) = p.bereits_abgerechnet_in.as_deref() {
        out.push(befund(
            tree,
            ebene,
            "A15",
            370,
            format!(
                "Diese Artikel-ID wurde für denselben Leistungszeitraum bereits mit der nicht \
                 stornierten RechnungsFakten {nummer} abgerechnet"
            ),
        ));
    }

    // 420 — Rechenfehler in der Position.
    if p.rechenfehler {
        out.push(befund(
            tree,
            ebene,
            "A20",
            420,
            "Menge × Einzelpreis ergibt nicht den ausgewiesenen Gesamtpreis der Position",
        ));
    }

    // 430 — the position-level catch-all.
    if let Some(detail) = p.sonstiger_fehler.as_ref() {
        out.push(befund(tree, ebene, "A99", 430, detail.clone()));
    }

    out
}

fn summenebene(tree: &'static str, r: &RechnungsFakten) -> Vec<Befund> {
    let mut out = Vec::new();
    let ebene = Ebene::Summe;

    // 510/520 — per (Steuersatz, Steuerkategorie). „Folgende Prüfungen sind je
    // Kombination aus Steuersatz und Steuerkategorie durchzuführen", and both
    // codes' Hinweis requires the pair to be named.
    for s in &r.steuersaetze {
        let bezeichnung = format!(
            "Steuersatz {} / Steuerkategorie {}",
            s.steuersatz, s.steuerkategorie
        );
        if !s.besteuerungsgrundlage_stimmt {
            out.push(befund(
                tree,
                ebene,
                "A22",
                510,
                format!(
                    "{bezeichnung}: die genannte Besteuerungsgrundlage passt nicht zur Summe der \
                     Einzelpositionen dieses Steuersatzes"
                ),
            ));
        }
        if !s.steuerbetrag_stimmt {
            out.push(befund(
                tree,
                ebene,
                "A23",
                520,
                format!(
                    "{bezeichnung}: der ausgewiesene Steuerbetrag entspricht nicht der Summe der \
                     Rechnungspositionen dieses Steuersatzes multipliziert mit dem Steuersatz"
                ),
            ));
        }
    }

    // 540 — Rechnungsbetrag = Σ Besteuerungsgrundlage + Σ Steuerbetrag.
    if !r.rechnungsbetrag_stimmt {
        out.push(befund(
            tree,
            ebene,
            "A24",
            540,
            "Der Rechnungsbetrag (Besteuerungsgrundlage inklusive Steuerbetrag) entspricht nicht \
             der Summe aller Rechnungspositionen",
        ));
    }

    // 550 — the Summen-level catch-all.
    if let Some(detail) = r.sonstiger_summenfehler.as_ref() {
        out.push(befund(tree, ebene, "A96", 550, detail.clone()));
    }

    out
}

// ── E_0265 — Nicht-Zahlungsavis prüfen (MSB) ─────────────────────────────────

/// What the MSB does with an ESA's Nicht-Zahlungsavis (`E_0265`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum NichtZahlungsavisAntwort {
    /// The refusal was justified — the MSB sends the **Stornorechnung 31004**
    /// and then a corrected invoice. WiM Teil 2 UC 4.5.1: „Eine
    /// Rechnungskorrektur umfasst immer eine Stornorechnung und eine neue
    /// Rechnung." No answer code: the Storno *is* the answer.
    Stornieren,
    /// The refusal was not justified — **COMDIS 29001** with `A99`, and the
    /// original invoice stands („Da dadurch die im Prozessschritt 1 versendete
    /// Rechnung weiterhin Bestand hat, ist keine neue Rechnung zu versenden").
    Widersprechen {
        /// `SG7 AJT` — `A99` and its tree.
        #[serde(flatten)]
        antwort: AntwortDetail,
        /// The Begründung the code's Hinweis requires („Es ist zu begründen,
        /// warum die Rechnung korrekt ist").
        begruendung: String,
    },
}

impl NichtZahlungsavisAntwort {
    /// The COMDIS Prüfidentifikator, or `None` when the answer is the Storno.
    #[must_use]
    pub const fn comdis_pid(&self) -> Option<u32> {
        match self {
            Self::Stornieren => None,
            Self::Widersprechen { .. } => Some(29_001),
        }
    }
}

/// Walk `E_0265` — the MSB's single Prüfschritt on an inbound Nicht-Zahlungsavis.
///
/// `begruendung` is only read on the [`NichtZahlungsavisAntwort::Widersprechen`]
/// branch, where the code's own Hinweis makes it mandatory.
///
/// # Panics
///
/// Only if the `E_0265` Codeliste is missing `A99`, which a test rules out.
#[must_use]
pub fn pruefe_nicht_zahlungsavis(
    familie: RechnungsFamilie,
    ablehnung_gerechtfertigt: bool,
    begruendung: impl Into<String>,
) -> NichtZahlungsavisAntwort {
    if ablehnung_gerechtfertigt {
        return NichtZahlungsavisAntwort::Stornieren;
    }
    let tree = familie.nicht_zahlungsavis;
    let code = lookup(tree, "A99").expect("A99 is published by every Nicht-Zahlungsavis tree");
    NichtZahlungsavisAntwort::Widersprechen {
        antwort: AntwortDetail::new(tree, code),
        begruendung: begruendung.into(),
    }
}

// ── E_0267 — Prüfen, ob Antwort auf Stornierung erforderlich (ESA) ───────────

/// How the ESA answered the invoice this Storno cancels — Prüfschritte 70/80.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UrsprungsAntwort {
    /// A Zahlungsavis went out (33001). The Storno is confirmed and booked.
    Zugestimmt,
    /// A Nicht-Zahlungsavis went out (33002/33003/33004). „Dann ist auf die
    /// Stornorechnung **keine** Antwort zu senden."
    Abgelehnt,
    /// The original invoice was never answered. „Dann ist weder auf die
    /// Rechnung noch auf die Stornorechnung eine Antwort zu senden."
    Unbeantwortet,
}

/// What `E_0267` decided about an inbound Stornorechnung.
///
/// Three outcomes, not two: the tree's name is „Prüfen, **ob** Antwort auf
/// Stornierung erforderlich", and its most common exit is that none is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum StornoAntwort {
    /// Prüfschritt 70 — „Stornorechnung zustimmen und im Zahlungslauf
    /// berücksichtigen". REMADV 33001, no `AJT`.
    Zustimmen,
    /// Prüfschritte 10–60 — REMADV 33002 with the code.
    Ablehnen {
        /// `SG7 AJT` — the `E_0267` code and its tree.
        #[serde(flatten)]
        antwort: AntwortDetail,
        /// The published Prüfschritt number.
        pruefschritt: u16,
        /// Human-readable explanation; the `FTX+ABO` text on `A99`.
        detail: String,
    },
    /// Prüfschritt 80 — **no message at all**. Sending a REMADV here answers a
    /// message the MSB is not waiting on.
    KeineAntwort {
        /// Why nothing is owed.
        grund: &'static str,
    },
}

impl StornoAntwort {
    /// The REMADV Prüfidentifikator, or `None` when no answer is owed.
    #[must_use]
    pub const fn remadv_pid(&self) -> Option<u32> {
        match self {
            Self::Zustimmen => Some(33_001),
            Self::Ablehnen { .. } => Some(33_002),
            Self::KeineAntwort { .. } => None,
        }
    }
}

/// Everything `E_0267` asks about an inbound Stornorechnung (INVOIC 31004).
#[allow(clippy::struct_excessive_bools)] // one field per Prüfschritt, by design
#[derive(Debug, Clone, PartialEq)]
pub struct StornoFakten {
    /// Prüfschritt 10 — is the invoice being cancelled on the ESA's books?
    pub ursprungsrechnung_bekannt: bool,
    /// Prüfschritt 15 — is the Storno's own Rechnungsnummer already on file
    /// from this Rechnungssteller?
    pub rechnungsnummer_bereits_verwendet: bool,
    /// Prüfschritt 17 — § 14 Abs. 4 UStG. `None` is „not assessed".
    pub ustg_konform: Option<bool>,
    /// Prüfschritt 20 — was the original already cancelled?
    pub bereits_storniert: bool,
    /// Prüfschritt 30 — same Rechnungstyp as the original?
    pub rechnungstyp_identisch: bool,
    /// Prüfschritt 40 — same Abrechnungszeitraum / Ausführungsdatum?
    pub zeitraum_identisch: bool,
    /// Prüfschritt 50 — does every `MOA` amount equal the original's × (−1)?
    pub betraege_negiert_identisch: bool,
    /// Prüfschritt 60 — a defect no earlier Prüfschritt names.
    pub sonstiger_fehler: Option<String>,
    /// Prüfschritte 70/80 — how the original invoice was answered.
    pub ursprungsantwort: UrsprungsAntwort,
}

/// Walk `E_0267` — the ESA's check of an inbound Stornorechnung.
///
/// # Panics
///
/// Only if the `E_0267` Codeliste is missing a code this function names, which
/// a test in this module rules out.
#[must_use]
pub fn pruefe_stornorechnung(familie: RechnungsFamilie, s: &StornoFakten) -> StornoAntwort {
    let tree = familie.storno;
    let refuse = |code: &str, pruefschritt: u16, detail: &str| StornoAntwort::Ablehnen {
        antwort: AntwortDetail::new(
            tree,
            lookup(tree, code).expect("code is published by every Storno tree"),
        ),
        pruefschritt,
        detail: detail.to_owned(),
    };

    if !s.ursprungsrechnung_bekannt {
        return refuse(
            "A01",
            10,
            "Die zu stornierende RechnungsFakten ist nicht vorhanden",
        );
    }
    if s.rechnungsnummer_bereits_verwendet {
        return refuse(
            "A06",
            15,
            "Die Rechnungsnummer der StornoFakten liegt von diesem Rechnungssteller bereits vor",
        );
    }
    if s.ustg_konform == Some(false) {
        return refuse(
            "A07",
            17,
            "Die StornoFakten erfüllt die Anforderungen des § 14 Abs. 4 UStG nicht",
        );
    }
    if s.bereits_storniert {
        return refuse(
            "A02",
            20,
            "Die zu stornierende RechnungsFakten wurde bereits storniert",
        );
    }
    if !s.rechnungstyp_identisch {
        return refuse(
            "A03",
            30,
            "Der Rechnungstyp der StornoFakten ist nicht identisch mit dem der ursprünglichen \
             RechnungsFakten",
        );
    }
    if !s.zeitraum_identisch {
        return refuse(
            "A04",
            40,
            "Der Abrechnungszeitraum bzw. das Ausführungsdatum der StornoFakten ist nicht \
             identisch mit dem der ursprünglichen RechnungsFakten",
        );
    }
    if !s.betraege_negiert_identisch {
        return refuse(
            "A05",
            50,
            "Mindestens ein Betrag der StornoFakten passt nicht zum Betrag der ursprünglichen \
             RechnungsFakten",
        );
    }
    if let Some(detail) = s.sonstiger_fehler.as_deref() {
        return refuse("A99", 60, detail);
    }

    // 70/80 — the Storno is sound; whether an answer is owed depends on how
    // the original was answered.
    match s.ursprungsantwort {
        UrsprungsAntwort::Zugestimmt => StornoAntwort::Zustimmen,
        UrsprungsAntwort::Abgelehnt => StornoAntwort::KeineAntwort {
            grund: "Die ursprüngliche RechnungsFakten wurde mit einem Nicht-Zahlungsavis abgelehnt — auf \
                    die StornoFakten ist keine Antwort zu senden (E_0267 Prüfschritt 80)",
        },
        UrsprungsAntwort::Unbeantwortet => StornoAntwort::KeineAntwort {
            grund: "Die ursprüngliche RechnungsFakten wurde noch nicht beantwortet — weder auf sie noch \
                    auf die StornoFakten ist eine Antwort zu senden (E_0267 Prüfschritt 80)",
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    const CAL: crate::HolidayCalendar = crate::HolidayCalendar::BdewMaKo;

    fn position(nr: u16) -> PositionsFakten {
        PositionsFakten {
            positionsnummer: nr,
            artikel_id: Some("9990001100002".to_owned()),
            artikel_id_aus_bestellung: true,
            leistung_erbracht: Some(true),
            preis_wie_angebot: Some(true),
            steuersatz_korrekt: Some(true),
            zeitraum: Some(Zeitraum {
                von: date!(2026 - 03 - 01),
                bis: date!(2026 - 03 - 31),
            }),
            bereits_abgerechnet_in: None,
            rechenfehler: false,
            sonstiger_fehler: None,
        }
    }

    fn rechnung() -> RechnungsFakten {
        RechnungsFakten {
            einwaende_entkraeftet: None,
            ustg_konform: Some(true),
            rechnungsdatum: date!(2026 - 04 - 01),
            eingangsdatum: date!(2026 - 04 - 01),
            leistungszeitraum: Some(Zeitraum {
                von: date!(2026 - 03 - 01),
                bis: date!(2026 - 03 - 31),
            }),
            bestellung_bekannt: true,
            rechnungsnummer_bereits_verwendet: false,
            faelliger_betrag_nicht_negativ: true,
            // 01.04.2026 + 10 WT (BdewMaKo: Karfreitag 03.04. and Ostermontag
            // 06.04.2026 are Feiertage) — well inside.
            zahlungsziel: Some(date!(2026 - 05 - 15)),
            preisblatt_version_gueltig: None,
            zeitraum_bereits_abgerechnet_in: None,
            sonstiger_kopffehler: None,
            positionen: vec![position(1)],
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

    /// A clean invoice is a Zahlungsavis, and the Zahlungsavis carries no code:
    /// REMADV 33001 has no `AJT` at all.
    #[test]
    fn a_clean_invoice_is_a_zahlungsavis_with_no_code() {
        let a = pruefe_rechnung(ESA, &rechnung(), CAL);
        assert!(a.ist_zustimmung());
        assert_eq!(a.remadv_pid(), 33_001);
        assert!(a.codes().is_empty());
    }

    /// „Führt eine Prüfung zu einer Ablehnung, werden keine weiteren
    /// Prüfschritte mehr durchgeführt" — a Kopf refusal is the *only* code, even
    /// when the positions are broken too.
    #[test]
    fn a_kopf_refusal_ends_the_walk() {
        let mut r = rechnung();
        r.bestellung_bekannt = false;
        r.positionen[0].rechenfehler = true;
        r.rechnungsbetrag_stimmt = false;
        let a = pruefe_rechnung(ESA, &r, CAL);
        assert_eq!(a.codes(), vec!["A04"]);
        assert_eq!(a.befunde[0].ebene, Ebene::Kopf);
        assert_eq!(a.befunde[0].pruefschritt, 40);
        // Kopf und Summe ride the same Prüfidentifikator.
        assert_eq!(a.remadv_pid(), 33_003);
    }

    /// Prüfschritt 70's own Hinweis: „Zahlungsziel ≤ 10 WT zum
    /// Rechnungseingangsdatum" is a refusal — the MSB had the same rule from
    /// UC 4.5.2 Nr. 1 and undercut it.
    #[test]
    fn a_short_zahlungsziel_is_refused() {
        let mut r = rechnung();
        r.eingangsdatum = date!(2026 - 04 - 01);
        r.zahlungsziel = Some(date!(2026 - 04 - 08));
        let a = pruefe_rechnung(ESA, &r, CAL);
        assert_eq!(a.codes(), vec!["A07"]);
        assert_eq!(a.befunde[0].pruefschritt, 70);
    }

    /// „Alle im Positionsteil gefundenen Fehler sind, unter Nennung der
    /// jeweiligen Positionszeile, zu nennen" — every defect of a position is
    /// reported, and the answer rides 33004 „Abweisung Position".
    #[test]
    fn position_defects_are_all_reported_under_their_positionsnummer() {
        let mut r = rechnung();
        r.positionen[0].preis_wie_angebot = Some(false);
        r.positionen[0].rechenfehler = true;
        let a = pruefe_rechnung(ESA, &r, CAL);
        assert_eq!(a.codes(), vec!["A11", "A20"]);
        assert!(a.befunde.iter().all(|b| b.ebene == Ebene::Position(1)));
        assert_eq!(a.remadv_pid(), 33_004);
    }

    /// Prüfschritt 300 „nein → 440": a position with no Artikel-ID from the
    /// Bestellung is refused once and its remaining Prüfschritte are skipped —
    /// they all compare against an Angebot position that was never identified.
    #[test]
    fn a_position_without_the_ordered_artikel_id_is_refused_once() {
        let mut r = rechnung();
        r.positionen[0].artikel_id = None;
        r.positionen[0].rechenfehler = true;
        let a = pruefe_rechnung(ESA, &r, CAL);
        assert_eq!(a.codes(), vec!["A09"]);
    }

    /// Prüfschritt 360 is a property of the position list, so the tree computes
    /// it: two positions billing one Artikel-ID for overlapping periods.
    #[test]
    fn a_duplicated_artikel_id_in_one_invoice_is_found_by_the_walk() {
        let mut r = rechnung();
        r.positionen.push(position(2));
        let a = pruefe_rechnung(ESA, &r, CAL);
        assert_eq!(a.codes(), vec!["A14", "A14"]);
        assert_eq!(a.befunde[0].ebene, Ebene::Position(1));
        assert_eq!(a.befunde[1].ebene, Ebene::Position(2));
    }

    /// …and two positions for the *same* Artikel-ID in disjoint periods are
    /// two legitimate months, not a duplicate.
    #[test]
    fn disjoint_periods_for_one_artikel_id_are_not_a_duplicate() {
        let mut r = rechnung();
        r.leistungszeitraum = Some(Zeitraum {
            von: date!(2026 - 02 - 01),
            bis: date!(2026 - 03 - 31),
        });
        let mut zweite = position(2);
        zweite.zeitraum = Some(Zeitraum {
            von: date!(2026 - 02 - 01),
            bis: date!(2026 - 02 - 28),
        });
        r.positionen.push(zweite);
        assert!(pruefe_rechnung(ESA, &r, CAL).ist_zustimmung());
    }

    /// Prüfschritt 500 ends the EBD on its own („ja → Ende"), and its Hinweis
    /// makes naming the missing Angebot positions part of the answer.
    #[test]
    fn missing_agreed_positions_end_the_walk_on_the_summenebene() {
        let mut r = rechnung();
        r.fehlende_artikel_ids = vec!["9990001100003".to_owned()];
        r.rechnungsbetrag_stimmt = false;
        let a = pruefe_rechnung(ESA, &r, CAL);
        assert_eq!(a.codes(), vec!["A21"]);
        assert!(a.befunde[0].detail.contains("9990001100003"));
        assert_eq!(a.remadv_pid(), 33_003);
    }

    /// 510/520 run per (Steuersatz, Steuerkategorie) and both codes' Hinweis
    /// requires the pair in the answer.
    #[test]
    fn the_tax_checks_run_per_rate_and_name_the_rate() {
        let mut r = rechnung();
        r.steuersaetze.push(Steuersatzpruefung {
            steuersatz: "0.00".to_owned(),
            steuerkategorie: "Z".to_owned(),
            besteuerungsgrundlage_stimmt: false,
            steuerbetrag_stimmt: false,
        });
        let a = pruefe_rechnung(ESA, &r, CAL);
        assert_eq!(a.codes(), vec!["A22", "A23"]);
        assert!(a.befunde[0].detail.contains("Steuerkategorie Z"));
    }

    /// `E_0266` is `E_0264` plus Prüfschritt 1, and `A25` is undefined in
    /// `E_0264` — so the second round cannot be answered from the first tree.
    #[test]
    fn the_second_round_adds_pruefschritt_one() {
        let mut r = rechnung();
        r.einwaende_entkraeftet = Some(false);
        // The first-round tree does not know the question at all.
        assert!(pruefe_rechnung(ESA, &r, CAL).ist_zustimmung());

        let a = pruefe_rechnung_erneut(ESA, &r, CAL);
        assert_eq!(a.tree, ESA.erneut);
        assert_eq!(a.codes(), vec!["A25"]);
        assert_eq!(a.befunde[0].pruefschritt, 1);
        assert!(lookup(ESA.rechnung, "A25").is_none());
    }

    /// A COMDIS 29001 must say why the invoice was right; the Storno branch
    /// sends no code at all.
    #[test]
    fn the_msb_either_cancels_or_states_its_case() {
        assert_eq!(
            pruefe_nicht_zahlungsavis(ESA, true, "irrelevant"),
            NichtZahlungsavisAntwort::Stornieren
        );
        assert_eq!(
            NichtZahlungsavisAntwort::Stornieren.comdis_pid(),
            None,
            "the Storno is the answer"
        );

        let widerspruch =
            pruefe_nicht_zahlungsavis(ESA, false, "Preis stammt aus dem Angebot vom …");
        assert_eq!(widerspruch.comdis_pid(), Some(29_001));
        let NichtZahlungsavisAntwort::Widersprechen { antwort, .. } = widerspruch else {
            panic!("expected a Widerspruch");
        };
        assert_eq!(antwort.antwortcode, "A99");
        assert_eq!(antwort.ebd.as_deref(), Some(ESA.nicht_zahlungsavis));
        assert!(antwort.braucht_bemerkung);
    }

    fn storno() -> StornoFakten {
        StornoFakten {
            ursprungsrechnung_bekannt: true,
            rechnungsnummer_bereits_verwendet: false,
            ustg_konform: Some(true),
            bereits_storniert: false,
            rechnungstyp_identisch: true,
            zeitraum_identisch: true,
            betraege_negiert_identisch: true,
            sonstiger_fehler: None,
            ursprungsantwort: UrsprungsAntwort::Zugestimmt,
        }
    }

    /// `E_0267`'s three outcomes. The one the name is about — „ob Antwort …
    /// erforderlich" — is the silent one, and sending a REMADV there answers a
    /// message the MSB is not waiting on.
    #[test]
    fn the_storno_answer_depends_on_how_the_original_was_answered() {
        assert_eq!(
            pruefe_stornorechnung(ESA, &storno()),
            StornoAntwort::Zustimmen
        );
        assert_eq!(StornoAntwort::Zustimmen.remadv_pid(), Some(33_001));

        for ohne in [UrsprungsAntwort::Abgelehnt, UrsprungsAntwort::Unbeantwortet] {
            let s = StornoFakten {
                ursprungsantwort: ohne,
                ..storno()
            };
            let a = pruefe_stornorechnung(ESA, &s);
            assert!(matches!(a, StornoAntwort::KeineAntwort { .. }), "{ohne:?}");
            assert_eq!(a.remadv_pid(), None);
        }
    }

    /// A refused Storno rides **33002** — the plain Abweisung — because
    /// `E_0267` is the one tree of this family REMADV AHB 1.0a § 3.1.1 admits
    /// in `SG7 AJT` DE 1082.
    #[test]
    fn a_refused_storno_rides_the_plain_abweisung() {
        let s = StornoFakten {
            bereits_storniert: true,
            ..storno()
        };
        let a = pruefe_stornorechnung(ESA, &s);
        assert_eq!(a.remadv_pid(), Some(33_002));
        let StornoAntwort::Ablehnen {
            antwort,
            pruefschritt,
            ..
        } = a
        else {
            panic!("expected an Ablehnung");
        };
        assert_eq!(antwort.antwortcode, "A02");
        assert_eq!(pruefschritt, 20);
    }

    /// Every code these walks can emit must be published by the tree it names —
    /// the guard that keeps an `E_0406` code off an ESA answer.
    #[test]
    fn every_emitted_code_is_published_by_its_tree() {
        for (tree, codes) in [
            (
                ESA.rechnung,
                &[
                    "A01", "A02", "A03", "A04", "A05", "A06", "A07", "A90", "A09", "A10", "A11",
                    "A12", "A13", "A14", "A15", "A20", "A99", "A21", "A22", "A23", "A24", "A96",
                ][..],
            ),
            (
                ESA.erneut,
                &[
                    "A25", "A01", "A02", "A03", "A04", "A05", "A06", "A07", "A90", "A09", "A10",
                    "A11", "A12", "A13", "A14", "A15", "A20", "A99", "A21", "A22", "A23", "A24",
                    "A96",
                ][..],
            ),
            (ESA.nicht_zahlungsavis, &["A99"][..]),
            (
                ESA.storno,
                &["A01", "A02", "A03", "A04", "A05", "A06", "A07", "A99"][..],
            ),
        ] {
            for code in codes {
                assert!(
                    lookup(tree, code).is_some(),
                    "{tree} does not publish {code}"
                );
            }
        }
    }

    /// `E_0264` and `E_0406` share catch-all letters and mean different things:
    /// `A99` is „sonstiger Fehler auf Positionsebene" in both, but `A24` is the
    /// ESA total check where `E_0406` uses `A70`. Resolving a code without
    /// naming its tree is what this guards.
    #[test]
    fn the_esa_total_check_is_not_the_netznutzung_one() {
        assert!(lookup(ESA.rechnung, "A70").is_none());
        assert!(lookup(crate::codes::EBD_NETZNUTZUNGSRECHNUNG, "A24").is_none());
    }

    // ── Family-exhaustive guards ─────────────────────────────────────────────
    //
    // The point of one engine over three copies: these run over every family, so
    // a tree whose alphabet drifts from the walk is caught for all of them at
    // once.

    /// Every code the walk can emit must be published by **every** family's
    /// tree — otherwise `befund`'s `expect` is a panic waiting for the first
    /// invoice that reaches that Prüfschritt.
    #[test]
    fn every_family_publishes_every_code_the_walk_names() {
        const KOPF: &[&str] = &["A01", "A02", "A03", "A04", "A05", "A06", "A07", "A90"];
        const POSITION: &[&str] = &[
            "A09", "A10", "A11", "A12", "A13", "A14", "A15", "A20", "A99",
        ];
        const SUMME: &[&str] = &["A21", "A22", "A23", "A24", "A96"];

        for f in FAMILIEN {
            for tree in [f.rechnung, f.erneut] {
                for c in KOPF.iter().chain(POSITION).chain(SUMME) {
                    assert!(
                        lookup(tree, c).is_some(),
                        "{tree} does not publish {c}, which the walk emits"
                    );
                }
                if f.hat_preisblatt_pruefung {
                    for c in ["A08", "A25"] {
                        assert!(
                            lookup(tree, c).is_some(),
                            "{tree} has the Preisblatt-Prüfschritte but does not publish {c}"
                        );
                    }
                }
            }
            // The second round's own Prüfschritt 1, and it is family-specific.
            assert!(
                lookup(f.erneut, f.erneut_einwand_code).is_some(),
                "{} does not publish {}",
                f.erneut,
                f.erneut_einwand_code
            );
            // …and the first round must NOT publish it, or the two rounds are
            // indistinguishable.
            assert!(
                lookup(f.rechnung, f.erneut_einwand_code).is_none()
                    || f.erneut_einwand_code == "A25" && f.hat_preisblatt_pruefung,
                "{} publishes the second round's {}",
                f.rechnung,
                f.erneut_einwand_code
            );
            assert!(lookup(f.nicht_zahlungsavis, "A99").is_some());
            for c in ["A01", "A02", "A03", "A04", "A05", "A06", "A07", "A99"] {
                assert!(
                    lookup(f.storno, c).is_some(),
                    "{} does not publish the Storno code {c}",
                    f.storno
                );
            }
        }
    }

    /// **The trap the shared engine exists to prevent.** „Konnte der MSB alle
    /// Einwände entkräften?" is Prüfschritt 1 of every second round and the
    /// BDEW answers it with a different code per family — `A25` for the ESA,
    /// `AC1` for Preisblatt B, where `A25` is already the doppelter
    /// Abrechnungszeitraum at Prüfschritt 90.
    #[test]
    fn the_second_round_code_is_family_specific_and_a25_is_overloaded() {
        assert_eq!(ESA.erneut_einwand_code, "A25");
        assert_eq!(PREISBLATT_B_LF.erneut_einwand_code, "AC1");
        assert_eq!(PREISBLATT_B_NB.erneut_einwand_code, "AC1");

        // `AC1` exists only in the Preisblatt-B second rounds.
        assert!(lookup(ESA.erneut, "AC1").is_none());
        assert!(lookup(PREISBLATT_B_LF.erneut, "AC1").is_some());

        // `A25` is in both families and means different things. The ESA's is
        // the Einwand; the Preisblatt-B one is the doppelter Zeitraum, and it
        // is in the *first* round too — which `AC1` never is.
        let esa = lookup(ESA.erneut, "A25").expect("E_0266 publishes A25");
        let pb = lookup(PREISBLATT_B_LF.rechnung, "A25").expect("E_0270 publishes A25");
        assert_ne!(esa.bedeutung, pb.bedeutung);
        assert!(lookup(ESA.rechnung, "A25").is_none(), "E_0264 has no A25");
    }

    /// The two Preisblatt-only Kopf-Prüfschritte, and the `A90` shift they
    /// cause. Running the same invoice through both families must produce the
    /// ESA's silence and the Preisblatt-B family's refusal.
    #[test]
    fn the_preisblatt_pruefschritte_are_family_gated() {
        assert_eq!(ESA.sonstiger_kopffehler_schritt(), 90);
        assert_eq!(PREISBLATT_B_LF.sonstiger_kopffehler_schritt(), 100);

        let mut r = rechnung();
        r.preisblatt_version_gueltig = Some(false);
        assert!(
            pruefe_rechnung(ESA, &r, CAL).ist_zustimmung(),
            "an ESA has no Preisblatt — its trees publish no such Prüfschritt"
        );
        let a = pruefe_rechnung(PREISBLATT_B_LF, &r, CAL);
        assert_eq!(a.antwortcodes(), vec!["A08"]);
        assert_eq!(a.befunde[0].pruefschritt, 80);

        let mut r = rechnung();
        r.zeitraum_bereits_abgerechnet_in = Some("RE-2026-0007".to_owned());
        assert!(pruefe_rechnung(ESA, &r, CAL).ist_zustimmung());
        let a = pruefe_rechnung(PREISBLATT_B_NB, &r, CAL);
        assert_eq!(a.antwortcodes(), vec!["A25"]);
        assert_eq!(a.befunde[0].pruefschritt, 90);
        assert!(
            a.befunde[0].detail.contains("RE-2026-0007"),
            "the Hinweis requires the earlier Rechnungsnummer to be named"
        );
    }

    /// `A90` moves with the family, because the two extra steps push it down.
    /// An auditor matches the Befund against the published row by this number.
    #[test]
    fn the_kopf_catch_all_reports_the_family_s_pruefschritt() {
        for (f, want) in [(ESA, 90u16), (PREISBLATT_B_LF, 100), (PREISBLATT_B_NB, 100)] {
            let mut r = rechnung();
            r.sonstiger_kopffehler = Some("Rechnungssteller unbekannt".to_owned());
            let a = pruefe_rechnung(f, &r, CAL);
            assert_eq!(a.antwortcodes(), vec!["A90"], "{}", f.rechnung);
            assert_eq!(a.befunde[0].pruefschritt, want, "{}", f.rechnung);
        }
    }

    /// The shared walk must behave identically on every family for the steps
    /// they share — that is the claim "one engine" makes.
    #[test]
    fn the_shared_pruefschritte_behave_alike_across_families() {
        for f in FAMILIEN {
            assert!(pruefe_rechnung(*f, &rechnung(), CAL).ist_zustimmung());

            let mut r = rechnung();
            r.rechnungsnummer_bereits_verwendet = true;
            let a = pruefe_rechnung(*f, &r, CAL);
            assert_eq!(a.antwortcodes(), vec!["A05"], "{}", f.rechnung);
            assert_eq!(a.remadv_pid(), 33_003);

            let mut r = rechnung();
            r.positionen[0].rechenfehler = true;
            let a = pruefe_rechnung(*f, &r, CAL);
            assert_eq!(a.antwortcodes(), vec!["A20"], "{}", f.rechnung);
            assert_eq!(a.remadv_pid(), 33_004, "position defects ride 33004");
        }
    }

    /// `familie_fuer` must agree with the tree table it bridges to.
    #[test]
    fn the_family_bridge_agrees_with_the_code_table() {
        use crate::codes::MsbRechnungsgegenstand as G;
        use mako_fristen::vorlauf::RechnungEmpfaenger as R;
        for (empf, geg, want) in [
            (R::Esa, G::Messstellenbetrieb, Some(ESA)),
            (R::LieferantOderMsb, G::PreisblattB, Some(PREISBLATT_B_LF)),
            (R::Netzbetreiber, G::PreisblattB, Some(PREISBLATT_B_NB)),
            // `E_0566`/`E_0210` carry their Codelisten but not their walk: the
            // Prüfschritte are their own (37 against `E_0264`'s 26, agreeing on
            // two of twenty shared numbers), and `E_0211` has no tree at all,
            // so `nicht_zahlungsavis` has no value to take.
            (R::Netzbetreiber, G::Messstellenbetrieb, None),
            (R::LieferantOderMsb, G::Messstellenbetrieb, None),
        ] {
            assert_eq!(familie_fuer(31_009, empf, geg), want, "{empf:?}/{geg:?}");
        }
        assert_eq!(familie_fuer(31_004, R::Esa, G::Messstellenbetrieb), None);
    }

    /// The Storno walk is step-for-step identical across families — `E_0267`
    /// and `E_0272`/`E_0275` publish the same eight codes at the same
    /// Prüfschritte.
    #[test]
    fn the_storno_walk_is_identical_across_families() {
        for f in FAMILIEN {
            assert_eq!(
                pruefe_stornorechnung(*f, &storno()),
                StornoAntwort::Zustimmen
            );

            let mut sr = storno();
            sr.bereits_storniert = true;
            let a = pruefe_stornorechnung(*f, &sr);
            let StornoAntwort::Ablehnen {
                antwort,
                pruefschritt,
                ..
            } = &a
            else {
                panic!("expected an Ablehnung for {}", f.storno);
            };
            assert_eq!(antwort.antwortcode, "A02");
            assert_eq!(*pruefschritt, 20);
            assert_eq!(antwort.tree, f.storno);
            assert_eq!(a.remadv_pid(), Some(33_002));
        }
    }
}
