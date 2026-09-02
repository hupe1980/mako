//! §52 EEG 2023 — deriving the Pflichtverstöße from the plant record.
//!
//! §52 Abs. 1 lists twelve violations, each charging the operator 10 €/kW per
//! calendar month (Abs. 2), reduced to 2 € once the obligation is met (Abs. 3),
//! capped at 10 €/kW in total (Abs. 5) and nettable against the Vergütung
//! (Abs. 6).
//!
//! `eeg-billing` models all twelve. This module is the one place that turns
//! plant facts into violations, so a rule cannot be half-present — detected by an
//! MCP tool but never reaching a settlement, or indexed but never queried.
//!
//! ## What is derived here, and what is not
//!
//! | § | Violation | Derivable from the plant record? |
//! |---|---|---|
//! | Abs. 1 Nr. 1 | §9 Steuerbarkeit missing | yes — `sect9_erfuellung` × capacity |
//! | Abs. 1 Nr. 4 | §10b Direktvermarktungspflicht | yes — capacity × settlement model |
//! | Abs. 1 Nr. 5 | Ausfallvergütung Höchstdauer exceeded | yes — from the receipts |
//! | Abs. 1 Nr. 9 | §21c switch not notified | yes — the notification timestamp |
//! | Abs. 1 Nr. 11 | MaStR registration missing | yes — `mastr_registriert` |
//! | Abs. 1 Nr. 2, 3, 6, 7, 8, 9a, 10, 12 | — | no: they turn on facts `einsd` does not hold (storage behaviour, metering resolution, Doppelvermarktung). Record them through the plant's `notes` and settle a correction. |

use eeg_billing::{Pflichtverstoss, SanktionsTyp};
use rust_decimal::{Decimal, dec};
use time::Date;

use crate::models;
use crate::pg::AnlageRow;

/// Installed capacity from which Direktvermarktung is mandatory.
///
/// §21 Abs. 1 Satz 1 Nr. 1 grants the Einspeisevergütung only up to 100 kW, so a
/// larger plant has to market directly; §10b carries the duty and §52 Abs. 1
/// Nr. 4 the charge for breaching it.
pub const DIREKTVERMARKTUNG_PFLICHT_KW: Decimal = dec!(100);

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

/// Derive every §52 Abs. 1 violation this plant is in for the billing period.
///
/// Returns an empty vector for a compliant plant — and for every plant governed
/// by the pre-2023 regime, where a breach reduces the Vergütung instead of
/// charging a separate Pflichtzahlung (`SanktionAlt`, handled by the caller).
#[must_use]
pub fn derive_pflichtverstoesse(anlage: &AnlageRow, ctx: Sect52Context) -> Vec<Pflichtverstoss> {
    let leistung_kw = anlage.leistung_kwp;
    let art = eeg_billing::ErzeugungsArt::from_db_str(&anlage.erzeugungsart).ok();
    let mut out = Vec::new();

    let mut push = |typ: SanktionsTyp, start: Option<Date>| {
        out.push(Pflichtverstoss {
            typ,
            leistung_kw,
            monate_des_verstosses: monate_seit(start, ctx.billing_date),
            nachtraeglich_erfuellt: false,
            technischer_defekt: false,
        });
    };

    // ── Nr. 1 — §9 Abs. 1/2 Steuerbarkeit ────────────────────────────────────
    // Staged by capacity: from 100 kW only Fernsteuerbarkeit will do, the
    // 25–100 kW band may take the 60 % Leistungsbegrenzung instead, and a
    // Steckersolargerät below 2 kW is out of scope. The old check was a flat
    // "≥ 25 kW without a Fernsteuerbarkeit date", which charged 10 €/kW/month to
    // every compliant plant that had taken the route the statute offers it.
    if eeg_billing::settlement_state::sect9_verletzt(leistung_kw, art, anlage.sect9_erfuellung()) {
        push(
            SanktionsTyp::FernsteuerbarkeitFehlend,
            anlage.fernsteuerbarkeit_violation_start,
        );
    }

    // ── Nr. 4 — §10b Direktvermarktungspflicht ───────────────────────────────
    // §21 Abs. 1 Satz 1 Nr. 1 grants the Einspeisevergütung only up to 100 kW, so
    // a larger plant taking it is in breach.
    //
    // Scoped to the plain Einspeisevergütung on purpose. MIETERSTROM and GGV are
    // *not* included: the Mieterstromzuschlag (§21 Abs. 3) and the §42b EnWG
    // supply relationship carry their own size rules, and reading the Abs. 1
    // Nr. 1 cap across to them is a contestable position on which to charge
    // 10 €/kW/month. The Ausfallvergütung is likewise excluded — it is the
    // statute's own answer for a plant whose Direktvermarkter dropped out, and
    // its limits are Nr. 5's business, not Nr. 4's.
    if leistung_kw > DIREKTVERMARKTUNG_PFLICHT_KW && anlage.settlement_model == models::VERGUETUNG {
        push(SanktionsTyp::DirektvermarktungspflichtVerletzt, None);
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

    out
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
