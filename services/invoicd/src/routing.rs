//! Which INVOIC Prüfidentifikator is checked how, and answered with what.
//!
//! # Why this is a table
//!
//! `invoicd` handled ten PIDs through four near-identical ~250-line functions.
//! They differed in five things — the check to run, the price sheet to fetch,
//! the accept/reject command names, an idempotency salt and an outcome label —
//! and agreed on everything else, including the parts that matter: persist
//! before dispatch, mark dispatched after, notify the ERP. They also drifted:
//! only two of the four notified the ERP at all, only two wrote a dead-letter
//! entry when the Rechnung would not parse, and one wrote it without the
//! tenant column, so every one of its inserts was rejected by the schema.
//!
//! Stating the five differences as data and running one pipeline over them
//! means a new PID is a row, and a change to the invariant is one edit rather
//! than four.
//!
//! # The PIDs
//!
//! | PID | Meaning | Check | Answer commands |
//! |---|---|---|---|
//! | 31001 | Abschlagsrechnung Netznutzung | NNE price sheet | `gpke.abrechnung.*` |
//! | 31002 | NN-Rechnung (Netznutzung, both Sparten) | NNE price sheet | `gpke.abrechnung.*` |
//! | 31003 | WiM-Rechnung (Dienstleistungen im Messwesen, **beide Sparten**) | `PreisblattMessung` | `wim.rechnung.*` |
//! | 31004 | Stornorechnung — **Sparte-neutral, any process** | arithmetic only | `invoic.stornorechnung.*` |
//! | 31005 | MMM-Rechnung Strom | NNE sheet + MMM Strom prices | `gpke.abrechnung.*` |
//! | 31006 | MMM Mehrmenge, selbst ausgestellt | NNE sheet + MMM Strom prices | `gpke.abrechnung.*` |
//! | 31007 | GaBi Gas MMM-Rechnung | NNE sheet + MMM Gas prices | `gabi.rechnung.*` |
//! | 31008 | GaBi Gas MMM, selbst ausgestellt | NNE sheet + MMM Gas prices | `gabi.rechnung.*` |
//! | 31009 | WiM MSB-Rechnung | `PreisblattMessung` + AufAbschlag | `wim.rechnung.*` |
//! | 31011 | Rechnung sonstige Leistung — **Sparte-neutral** (GPKE Teil 2 · AWH Sperrprozesse Gas) | NNE price sheet | `invoic.sonstige-leistung.*` |
//!
//! Sources: BDEW INVOIC AHB (Anwendungsübersicht Prüfidentifikatoren 4.0),
//! BK6-24-174; WiM Teil 1; GaBi Gas 2.1 (BK7-24-01-008); GeLi Gas 3.0
//! (BK7-24-01-009).

/// Which reference data the plausibility check needs, and which stages to run.
use mako_markt::commands;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckKind {
    /// Stages 1–5 against `PreisblattNetznutzung` from the sending NB.
    Netznutzung,
    /// [`Self::Netznutzung`] plus stage 6 against the nationwide monthly Strom
    /// Mehr-/Mindermengenpreise.
    ///
    /// The Strom prices are one BDEW series for the whole market (§ 13 Abs. 3
    /// StromNZV until 31.12.2025, GPKE Teil 1 Kap. 8.4 from 01.01.2026), so the
    /// application month alone identifies them — the sending NB is not part of
    /// the key.
    NetznutzungMitMmmStrom,
    /// [`Self::Netznutzung`] plus stage 6 against the Gas Mehr-/Mindermengen-
    /// Abrechnungspreise of the Marktgebietsverantwortlicher.
    ///
    /// Unlike Strom, Gas prices are per Marktgebiet; Trading Hub Europe is the
    /// single German MGV since the 2021 merger.
    NetznutzungMitMmmGas,
    /// `PreisblattMessung` plus the contracted AufAbschlag list (PID 31009).
    ///
    /// The MSB-Rechnung prices metering service, not network use, so validating
    /// it against `PreisblattNetznutzung` would compare unrelated tariffs.
    Messung,
    /// Storno reference, period and arithmetic only — no tariff lookup.
    ///
    /// A Stornorechnung carries the original's amounts negated, so a tariff
    /// comparison disputes every one of them.
    ArithmetikNur,
}

/// One PID's handling.
#[derive(Debug, Clone, Copy)]
pub struct PidRoute {
    /// The Prüfidentifikator.
    pub pid: u32,
    /// What the plausibility check needs.
    pub check: CheckKind,
    /// Command dispatched to `makod` when the invoice is accepted.
    pub accept: &'static str,
    /// Command dispatched when it is disputed.
    pub reject: &'static str,
    /// Salt for the per-process idempotency key.
    ///
    /// Distinct per route so an accept and a later manual re-dispatch of the
    /// same process are separate commands in `makod`'s log rather than one.
    pub salt: &'static [u8],
}

/// Every INVOIC PID `invoicd` answers.
///
/// The order is the PID order; [`route_for`] does a linear scan, which over ten
/// entries costs less than the hash it would replace.
pub const ROUTES: &[PidRoute] = &[
    PidRoute {
        pid: 31001,
        check: CheckKind::Netznutzung,
        accept: commands::GPKE_ABRECHNUNG_ANNEHMEN,
        reject: commands::GPKE_ABRECHNUNG_ABLEHNEN,
        salt: b"gpke",
    },
    PidRoute {
        pid: 31002,
        check: CheckKind::Netznutzung,
        accept: commands::GPKE_ABRECHNUNG_ANNEHMEN,
        reject: commands::GPKE_ABRECHNUNG_ABLEHNEN,
        salt: b"gpke",
    },
    PidRoute {
        pid: 31003,
        // The WiM-Rechnung bills *Dienstleistungen im Messwesen* — the temporäre
        // Fortführung, the Geräteübernahme, a Zwischen- oder Kontrollablesung —
        // between the abgebender and the aufnehmender MSB. That is metering
        // service, not network use, so it prices against `PreisblattMessung`
        // for the same reason PID 31009 does.
        check: CheckKind::Messung,
        // …and it belongs to the WiM billing family, which is what `makod`
        // registers. There is no `wim.gas.*` command: the Gas 31003 is answered
        // by `wim.rechnung.*` too, which is why that descriptor carries `Gnb`
        // among its permitted roles.
        accept: commands::WIM_RECHNUNG_ANNEHMEN,
        reject: commands::WIM_RECHNUNG_ABLEHNEN,
        salt: b"wim-dienstleistung",
    },
    PidRoute {
        pid: 31004,
        check: CheckKind::ArithmetikNur,
        accept: commands::INVOIC_STORNORECHNUNG_ANNEHMEN,
        reject: commands::INVOIC_STORNORECHNUNG_ABLEHNEN,
        salt: b"invoic-storno",
    },
    PidRoute {
        pid: 31005,
        check: CheckKind::NetznutzungMitMmmStrom,
        accept: commands::GPKE_ABRECHNUNG_ANNEHMEN,
        reject: commands::GPKE_ABRECHNUNG_ABLEHNEN,
        salt: b"gpke",
    },
    PidRoute {
        pid: 31006,
        check: CheckKind::NetznutzungMitMmmStrom,
        accept: commands::GPKE_ABRECHNUNG_ANNEHMEN,
        reject: commands::GPKE_ABRECHNUNG_ABLEHNEN,
        salt: b"gpke",
    },
    PidRoute {
        pid: 31007,
        check: CheckKind::NetznutzungMitMmmGas,
        accept: commands::GABI_RECHNUNG_ANNEHMEN,
        reject: commands::GABI_RECHNUNG_ABLEHNEN,
        salt: b"gabi-gas",
    },
    PidRoute {
        pid: 31008,
        check: CheckKind::NetznutzungMitMmmGas,
        accept: commands::GABI_RECHNUNG_ANNEHMEN,
        reject: commands::GABI_RECHNUNG_ABLEHNEN,
        salt: b"gabi-gas",
    },
    PidRoute {
        pid: 31009,
        check: CheckKind::Messung,
        accept: commands::WIM_RECHNUNG_ANNEHMEN,
        reject: commands::WIM_RECHNUNG_ABLEHNEN,
        salt: b"wim-msb",
    },
    PidRoute {
        pid: 31011,
        check: CheckKind::Netznutzung,
        accept: commands::INVOIC_SONSTIGE_LEISTUNG_ANNEHMEN,
        reject: commands::INVOIC_SONSTIGE_LEISTUNG_ABLEHNEN,
        salt: b"geli-gas",
    },
];

/// The route for `pid`, or `None` when this service does not answer it.
///
/// Returning `None` rather than a default route is deliberate: dispatching
/// `gpke.abrechnung.annehmen` for an unrecognised PID would accept an invoice
/// from a process this service does not understand.
#[must_use]
pub fn route_for(pid: u32) -> Option<&'static PidRoute> {
    ROUTES.iter().find(|r| r.pid == pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One route per PID. A duplicate would make `route_for` pick by table
    /// order, which is not a decision anyone made.
    #[test]
    fn every_pid_appears_exactly_once() {
        let mut pids: Vec<u32> = ROUTES.iter().map(|r| r.pid).collect();
        let before = pids.len();
        pids.sort_unstable();
        pids.dedup();
        assert_eq!(pids.len(), before, "duplicate PID in ROUTES");
    }

    /// The commands must name the process family that owns the PID: a Gas WiM
    /// invoice answered with `gpke.abrechnung.annehmen` reaches no workflow.
    #[test]
    fn commands_match_the_owning_process_family() {
        for r in ROUTES {
            let expected = match r.pid {
                31001 | 31002 | 31005 | 31006 => "gpke.abrechnung.",
                31003 | 31009 => "wim.rechnung.",
                31004 => "invoic.stornorechnung.",
                31007 | 31008 => "gabi.rechnung.",
                31011 => "invoic.sonstige-leistung.",
                other => panic!("PID {other} has no expected command family"),
            };
            assert!(
                r.accept.starts_with(expected) && r.accept.ends_with("annehmen"),
                "PID {} accept command {:?}",
                r.pid,
                r.accept
            );
            assert!(
                r.reject.starts_with(expected) && r.reject.ends_with("ablehnen"),
                "PID {} reject command {:?}",
                r.pid,
                r.reject
            );
        }
    }

    /// A PID this service does not answer must not fall through to a default.
    #[test]
    fn an_unknown_pid_has_no_route() {
        assert!(route_for(0).is_none());
        assert!(route_for(31010).is_none(), "Gas Kapazität is not handled");
        assert!(route_for(55001).is_none(), "a UTILMD PID is not an INVOIC");
    }

    /// PID 31004 is Sparte-neutral and cancels invoices from every process
    /// family (GPKE, MMM, WiM Strom + Gas, Kapazität, AWH, GeLi Gas), so it
    /// runs the arithmetic-only check and answers with a neutral command.
    #[test]
    fn the_storno_pid_is_sparte_neutral() {
        let r = route_for(31004).expect("31004 is routed");
        assert_eq!(r.check, CheckKind::ArithmetikNur);
        assert!(!r.accept.contains("gas"), "{}", r.accept);
        assert!(!r.accept.contains("gpke"), "{}", r.accept);
    }

    /// 31002 is the NN-Rechnung, not an MMM PID: it prices network use against
    /// `PreisblattNetznutzung`. Adding it to the MMM set disputed every line of
    /// every Netznutzungsrechnung against Mehr-/Mindermengenpreise.
    #[test]
    fn the_nn_rechnung_is_not_checked_against_mmm_prices() {
        assert_eq!(
            route_for(31002).expect("routed").check,
            CheckKind::Netznutzung
        );
        for pid in [31005u32, 31006] {
            assert_eq!(
                route_for(pid).expect("routed").check,
                CheckKind::NetznutzungMitMmmStrom
            );
        }
        for pid in [31007u32, 31008] {
            assert_eq!(
                route_for(pid).expect("routed").check,
                CheckKind::NetznutzungMitMmmGas
            );
        }
    }

    /// The MSB-Rechnung prices metering service. Checking it against
    /// `PreisblattNetznutzung` compares unrelated tariffs and disputes a
    /// correct invoice.
    #[test]
    fn the_msb_rechnung_uses_the_metering_price_sheet() {
        assert_eq!(route_for(31009).expect("routed").check, CheckKind::Messung);
    }
}
