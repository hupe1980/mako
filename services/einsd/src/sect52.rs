//! §52 EEG 2023 — the Pflichtverstöße of one plant, from two sources.
//!
//! §52 Abs. 1 lists **thirteen** violations: it counts to twelve and inserts a
//! Nr. 9a between 9 and 10. Each charges 10 €/kW per calendar month (Abs. 2),
//! four of them run past the breach itself (Abs. 4), the total is capped at
//! 10 €/kW per month (Abs. 5), and the claim may be netted against the Vergütung
//! (Abs. 6). Abs. 3 reduces the rate, but only for the Nummern it names.
//!
//! This module is the one place that turns plant facts into violations, so a
//! rule cannot be half-present — detected by an MCP tool but never reaching a
//! settlement, or indexed but never queried.
//!
//! ## Two sources, one merge
//!
//! Four breaches follow from the plant record. The other nine turn on facts no
//! register row carries, and those are **recorded** in `eeg_pflichtverstoesse`
//! by the operator or the ERP — §10b Abs. 5 leaves the Nachweis to the
//! Netzbetreiber and the Direktvermarkter, §9 Abs. 5's Speicheranforderung is a
//! site visit, Doppelvermarktung is a finding.
//!
//! | § | Violation | Source |
//! |---|---|---|
//! | Abs. 1 Nr. 1 | §9 Steuerbarkeit missing | derived (`sect9_erfuellung` × capacity) |
//! | Abs. 1 Nr. 5 | Ausfallvergütung Höchstdauer exceeded | derived from the receipts |
//! | Abs. 1 Nr. 9 | §21c switch not notified | derived from the notification timestamp |
//! | Abs. 1 Nr. 11 | MaStR registration missing | derived; `mastr_violation_start` is its clock |
//! | Abs. 1 Nr. 2, 3, 4, 6, 7, 8, 9a, 10, 12 | — | **recorded** — the register entry is the trigger |
//!
//! A recorded entry never *contradicts* a derived one: where einsd derives the
//! breach, the record only refines it. An operator therefore cannot close a
//! Pflichtverstoß the plant record still shows.
//!
//! ## The register is the only source for three facts, and each moves money
//!
//! - **`beginn`** — Abs. 2 charges per calendar month „in dem ganz oder
//!   zeitweise ein Pflichtverstoß … vorliegt". No start date means one month,
//!   which understates a breach that has run for a year.
//! - **`behoben_am`** — Abs. 3 Satz 1 Nr. 1 drops the rate to 2 €/kW „zurück bis
//!   zum Beginn", so months already settled at 10 € need a § 147 AO correction.
//! - **`technischer_defekt`** — Abs. 3 Satz 2 waives the defect month and the
//!   next one for Nr. 1/3/4/8; the Beweislast is the operator's.
//!
//! Neither Abs. 3 reduction is derivable, so without a record both flags are
//! `false` — which means „nobody said", not „no reduction applies".
//!
use eeg_billing::{Pflichtverstoss, SanktionsTyp};
use rust_decimal::{Decimal, dec};
use time::Date;

use crate::models;
use crate::pg::{AnlageRow, PflichtverstossRecord};

/// §10b Abs. 1 Satz 1 EEG 2023 — the installed capacity from which a
/// direct-marketing plant owes the Abruf- und Fernsteuerbarkeit.
///
/// „Betreiber von Anlagen mit einer installierten Leistung von **mehr als 25
/// Kilowatt**, die den in ihren Anlagen erzeugten Strom direkt vermarkten".
/// Breaching it is §52 Abs. 1 Nr. 4.
pub const SECT10B_MINDESTLEISTUNG_KW: Decimal = dec!(25);

/// §21 Abs. 1 Satz 1 Nr. 3 — the two Höchstdauern of the Ausfallvergütung.
pub const AUSFALLVERGUETUNG_MAX_MONATE_AM_STUECK: u32 = 3;
/// Six calendar months per calendar year, consecutive or not.
pub const AUSFALLVERGUETUNG_MAX_MONATE_JAHR: u32 = 6;

/// How long a plant has already been on the Ausfallvergütung.
///
/// Both figures are counted **including** the period being settled, because
/// §21 Abs. 1 Satz 1 Nr. 3 caps the Inanspruchnahme itself, not the months after
/// it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AusfallverguetungNutzung {
    /// Consecutive months up to and including this one.
    pub monate_am_stueck: u32,
    /// Months in this calendar year, consecutive or not.
    pub monate_im_jahr: u32,
}

impl AusfallverguetungNutzung {
    /// Whether either Höchstdauer is exceeded — the §52 Abs. 1 Nr. 5 trigger.
    #[must_use]
    pub fn hoechstdauer_ueberschritten(self) -> bool {
        self.monate_am_stueck > AUSFALLVERGUETUNG_MAX_MONATE_AM_STUECK
            || self.monate_im_jahr > AUSFALLVERGUETUNG_MAX_MONATE_JAHR
    }
}

/// Everything about the period that §52 detection needs beyond the plant row.
#[derive(Debug, Clone, Copy)]
pub struct Sect52Context {
    /// First day of the billing month.
    pub billing_date: Date,
    /// How long the plant has been on the Ausfallvergütung, if it is.
    pub ausfallverguetung: AusfallverguetungNutzung,
}

/// Inclusive count of calendar months from `start` to `billing_date`.
///
/// A violation that began in the billing month itself counts as one month —
/// §52 Abs. 2 charges per calendar month "in dem der Verstoß ganz oder teilweise
/// vorliegt". An untracked start date is likewise one month rather than zero:
/// under-charging silently is no better than over-charging.
fn monate_seit(start: Option<Date>, billing_date: Date) -> u32 {
    let Some(start) = start else { return 1 };
    let years = billing_date.year() - start.year();
    let months = billing_date.month() as i32 - start.month() as i32;
    u32::try_from((years * 12 + months + 1).max(1)).unwrap_or(1)
}

/// Every §52 Abs. 1 violation this plant is in for the billing period.
///
/// `aufzeichnungen` is the plant's `eeg_pflichtverstoesse` history; only the
/// entries that bear on the billing month are read
/// ([`PflichtverstossRecord::gilt_fuer`]).
///
/// Returns an empty vector for a compliant plant — and for every plant governed
/// by the pre-2023 regime, where a breach reduces the Vergütung instead of
/// charging a separate Pflichtzahlung (`SanktionAlt`, handled by the caller).
#[must_use]
pub fn derive_pflichtverstoesse(
    anlage: &AnlageRow,
    aufzeichnungen: &[PflichtverstossRecord],
    ctx: Sect52Context,
) -> Vec<Pflichtverstoss> {
    let leistung_kw = anlage.leistung_kwp;
    let art = eeg_billing::ErzeugungsArt::from_db_str(&anlage.erzeugungsart).ok();
    let mut out = Vec::new();

    // Only the entries that bear on this month, indexed by Nummer.
    let relevant: Vec<&PflichtverstossRecord> = aufzeichnungen
        .iter()
        .filter(|r| r.gilt_fuer(ctx.billing_date))
        .collect();
    let aufzeichnung = |typ: SanktionsTyp| -> Option<&PflichtverstossRecord> {
        relevant.iter().copied().find(|r| r.typ == typ)
    };

    // One place builds a `Pflichtverstoss`, so the §52 Abs. 3 flags and the
    // Abs. 4 month extension cannot be applied to one Nummer and forgotten on
    // another.
    let mut push = |typ: SanktionsTyp, fallback_start: Option<Date>| {
        let record = aufzeichnung(typ);
        let start = record.map(|r| r.beginn).or(fallback_start);
        let bis = record
            .and_then(|r| r.behoben_am)
            .filter(|b| *b < ctx.billing_date)
            .unwrap_or(ctx.billing_date);
        out.push(Pflichtverstoss {
            typ,
            leistung_kw,
            monate_des_verstosses: typ.abs4_monate(monate_seit(start, bis)),
            // §52 Abs. 5 caps concurrent violations per Kalendermonat, so the
            // months a violation occupies have to be placeable in the calendar.
            beginn: start,
            // §52 Abs. 3 Satz 1 Nr. 1 — „sobald die entsprechende Pflicht
            // erfüllt wird", and the reduction reaches back to the beginning.
            nachtraeglich_erfuellt: record.is_some_and(|r| r.behoben_am.is_some()),
            technischer_defekt: record.is_some_and(|r| r.technischer_defekt),
        });
    };

    // ── Nr. 1 — §9 Abs. 1/2 Steuerbarkeit ────────────────────────────────────
    // Staged by capacity: from 100 kW only Fernsteuerbarkeit will do, the
    // 25–100 kW band may take the 60 % Leistungsbegrenzung instead, and a
    // Steckersolargerät below 2 kW is out of scope. The old check was a flat
    // "≥ 25 kW without a Fernsteuerbarkeit date", which charged 10 €/kW/month to
    // every compliant plant that had taken the route the statute offers it.
    if eeg_billing::settlement_state::sect9_verletzt(leistung_kw, art, anlage.sect9_erfuellung()) {
        push(SanktionsTyp::FernsteuerbarkeitFehlend, None);
    }

    // ── Nr. 5 — Ausfallvergütung beyond its Höchstdauern ─────────────────────
    if anlage.settlement_model == models::AUSFALLVERGUETUNG
        && ctx.ausfallverguetung.hoechstdauer_ueberschritten()
    {
        push(
            SanktionsTyp::AusfallverguetungHoechstdauerUeberschritten,
            None,
        );
    }

    // ── Nr. 9 — §21c Zuordnung/Wechsel not notified ──────────────────────────
    // The registry has carried an index for exactly this predicate since the
    // column was added, and nothing ever queried it.
    if anlage.last_veraeusserungsform_switch.is_some()
        && anlage.veraeusserungsform_notification_sent_at.is_none()
    {
        push(
            SanktionsTyp::ZuordnungsWechselNichtGemeldet,
            anlage.last_veraeusserungsform_switch,
        );
    }

    // ── Nr. 11 — MaStR registration missing ──────────────────────────────────
    if !anlage.mastr_registriert {
        push(
            SanktionsTyp::MastrNichtRegistriert,
            anlage.mastr_violation_start,
        );
    }

    // ── The nine recorded Nummern ────────────────────────────────────────────
    //
    // A record is the trigger for anything einsd cannot see. Nr. 4 is the one
    // that also carries a statutory *scope*: §10b Abs. 1 binds the operator of a
    // plant over 25 kW **that direct-markets**, so a record filed against a plant
    // on the Einspeisevergütung charges nothing — it breaches a duty it does not
    // have. Everything else is charged as filed.
    let derived: [SanktionsTyp; 4] = [
        SanktionsTyp::FernsteuerbarkeitFehlend,
        SanktionsTyp::AusfallverguetungHoechstdauerUeberschritten,
        SanktionsTyp::ZuordnungsWechselNichtGemeldet,
        SanktionsTyp::MastrNichtRegistriert,
    ];
    let in_direktvermarktung = matches!(
        anlage.settlement_model.as_str(),
        models::DIREKTVERMARKTUNG | models::AUSSCHREIBUNG | models::SONSTIGE_DIREKTVERMARKTUNG
    );
    let recorded: Vec<SanktionsTyp> = relevant
        .iter()
        .map(|r| r.typ)
        .filter(|typ| !derived.contains(typ))
        .filter(|typ| {
            *typ != SanktionsTyp::Sect10bVorgabenVerletzt
                || (in_direktvermarktung && leistung_kw > SECT10B_MINDESTLEISTUNG_KW)
        })
        .collect();
    for typ in recorded {
        push(typ, None);
    }

    out
}

/// What the plant owes for **one** calendar month if these breaches persist.
///
/// Not the total — [`Pflichtverstoss::monate_des_verstosses`] carries the months
/// already accrued (including the §52 Abs. 4 tail), and a compliance *report*
/// wants the monthly exposure rather than the running claim. Each violation is
/// therefore priced at a single month, the §52 Abs. 3 rate the engine picks is
/// kept, and the Abs. 5 ceiling — „insgesamt auf 10 Euro pro Kilowatt
/// installierter Leistung der Anlage und Kalendermonat begrenzt" — is applied to
/// the sum.
///
/// Lives here rather than at the report, so the figure an operator is shown and
/// the figure a settlement charges cannot come from two different formulas.
#[must_use]
pub fn monatliche_exposition(verstoesse: &[Pflichtverstoss], leistung_kw: Decimal) -> Decimal {
    verstoesse
        .iter()
        .map(|v| {
            eeg_billing::calculate_pflichtzahlung(&Pflichtverstoss {
                monate_des_verstosses: 1,
                beginn: None,
                ..v.clone()
            })
        })
        .sum::<Decimal>()
        .min(leistung_kw * dec!(10))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ausfallverguetung_limits_are_both_checked() {
        let ok = AusfallverguetungNutzung {
            monate_am_stueck: 3,
            monate_im_jahr: 6,
        };
        assert!(!ok.hoechstdauer_ueberschritten(), "both limits exactly met");
        assert!(
            AusfallverguetungNutzung {
                monate_am_stueck: 4,
                monate_im_jahr: 4,
            }
            .hoechstdauer_ueberschritten(),
            "four consecutive months breaches the first limit"
        );
        assert!(
            AusfallverguetungNutzung {
                monate_am_stueck: 1,
                monate_im_jahr: 7,
            }
            .hoechstdauer_ueberschritten(),
            "a seventh month in the year breaches the second, however spread out"
        );
    }

    /// §52 Abs. 2 charges per calendar month in which the breach subsists at all,
    /// so the month it started in counts.
    #[test]
    fn the_month_a_violation_started_in_counts_as_one() {
        use time::macros::date;
        assert_eq!(
            monate_seit(Some(date!(2026 - 06 - 01)), date!(2026 - 06 - 01)),
            1
        );
        assert_eq!(
            monate_seit(Some(date!(2026 - 06 - 15)), date!(2026 - 08 - 01)),
            3
        );
        assert_eq!(
            monate_seit(Some(date!(2025 - 11 - 01)), date!(2026 - 02 - 01)),
            4
        );
        assert_eq!(
            monate_seit(None, date!(2026 - 06 - 01)),
            1,
            "untracked start"
        );
        assert_eq!(
            monate_seit(Some(date!(2027 - 01 - 01)), date!(2026 - 06 - 01)),
            1,
            "a start in the future cannot produce a negative month count"
        );
    }
}
