//! ERP command names shared between `makod`'s command registry and its clients.
//!
//! `makod` exposes `POST /api/v1/commands` whose `command` field must match an
//! entry in its command registry — an unknown name is rejected with HTTP 422.
//! Services that dispatch commands (`processd`, `invoicd`, …) MUST use these
//! constants instead of string literals so the wire name cannot drift from the
//! registry: `makod` carries a registry test asserting every constant in
//! [`DISPATCHED_BY_SERVICES`] is registered.
//!
//! Only names actually posted by out-of-process callers are listed here; the
//! registry itself remains the single source of truth for roles, PIDs, and
//! dispatch functions.

// ── GPKE (electricity supplier processes) ─────────────────────────────────────

/// LF: initiate a Lieferbeginn Anmeldung (UTILMD 55001).
pub const GPKE_LIEFERBEGINN_ANMELDEN: &str = "gpke.lieferbeginn.anmelden";
/// NB: confirm an inbound Lieferbeginn Anmeldung (UTILMD 55002 / 55078).
pub const GPKE_LIEFERBEGINN_BESTAETIGEN: &str = "gpke.lieferbeginn.bestaetigen";
/// NB: confirm a Neuanlage — inbound 55600 / 55601, answered UTILMD 55602 /
/// 55603 (EBD `E_0608`, Zustimmung `A09` / `A18`).
pub const GPKE_NEUANLAGE_BESTAETIGEN: &str = "gpke.neuanlage.bestaetigen";
/// NB: refuse a Neuanlage — inbound 55600 / 55601, answered UTILMD 55604 /
/// 55605 (EBD `E_0608`).
pub const GPKE_NEUANLAGE_ABLEHNEN: &str = "gpke.neuanlage.ablehnen";
/// NB: assign a contractless `MaLo` to the Grundversorger (UTILMD 55013, §38 `EnWG`).
pub const GPKE_EOG_ANMELDEN: &str = "gpke.eog.anmelden";
/// NB: reject an inbound Lieferbeginn Anmeldung (UTILMD 55003 / 55080).
pub const GPKE_LIEFERBEGINN_ABLEHNEN: &str = "gpke.lieferbeginn.ablehnen";
/// LF: initiate a Lieferende Abmeldung (UTILMD 55004).
pub const GPKE_LIEFERENDE_ANMELDEN: &str = "gpke.lieferende.anmelden";
/// LFN: send the Kündigung to the Altlieferant — UTILMD 55016, answered
/// 55017 / 55018 (`E_0614`). The Gas twin is [`GELI_KUENDIGUNG_ANMELDEN`].
pub const GPKE_KUENDIGUNG_ANMELDEN: &str = "gpke.kuendigung.anmelden";
/// NB: confirm an inbound Abmeldung (inbound 55004 → UTILMD 55005, EBD `E_0607`).
pub const GPKE_LIEFERENDE_BESTAETIGEN: &str = "gpke.lieferende.bestaetigen";
/// NB: reject an inbound Abmeldung (inbound 55004 → UTILMD 55006, EBD `E_0607`).
pub const GPKE_LIEFERENDE_ABLEHNEN: &str = "gpke.lieferende.ablehnen";
/// LF: confirm an NB-initiated Lieferende — inbound 55007, answered
/// UTILMD 55008 (EBD `E_0609`).
pub const GPKE_NB_LIEFERENDE_BESTAETIGEN: &str = "gpke.nb-lieferende.bestaetigen";
/// LF: reject an NB-initiated Lieferende — inbound 55007, answered
/// UTILMD 55009 (EBD `E_0609`).
pub const GPKE_NB_LIEFERENDE_ABLEHNEN: &str = "gpke.nb-lieferende.ablehnen";
/// LFA: confirm an NB `Anfrage zur Beendigung der Zuordnung` (UTILMD 55011).
///
/// The inbound PID is 55010 and the EBD is **`E_0624`** ("Anfrage zur Beendigung
/// der Zuordnung prüfen") — distinct from the NB-seitiges Lieferende above
/// (55007 → 55008/55009, EBD `E_0609`).
pub const GPKE_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN: &str = "gpke.beendigung-zuordnung.bestaetigen";
/// LFA: reject an NB `Anfrage zur Beendigung der Zuordnung` (UTILMD 55012).
pub const GPKE_BEENDIGUNG_ZUORDNUNG_ABLEHNEN: &str = "gpke.beendigung-zuordnung.ablehnen";

// ── GeLi Gas ──────────────────────────────────────────────────────────────────

/// LF: initiate a gas Lieferbeginn Anmeldung (UTILMD 44001).
pub const GELI_LIEFERBEGINN_ANMELDEN: &str = "geli.lieferbeginn.anmelden";
/// NB: confirm an inbound gas Lieferbeginn Anmeldung.
pub const GELI_LIEFERBEGINN_BESTAETIGEN: &str = "geli.lieferbeginn.bestaetigen";
/// NB: reject an inbound gas Lieferbeginn Anmeldung.
pub const GELI_LIEFERBEGINN_ABLEHNEN: &str = "geli.lieferbeginn.ablehnen";
/// LF: initiate a gas Lieferende Abmeldung (UTILMD 44004).
pub const GELI_LIEFERENDE_ANMELDEN: &str = "geli.lieferende.anmelden";
/// LFG: send the Kündigung to the Altlieferant — UTILMD G 44016, answered
/// 44017 / 44018 (`E_3001`). BK7-24-01-009 § 3.1; the Strom twin is
/// [`GPKE_KUENDIGUNG_ANMELDEN`].
pub const GELI_KUENDIGUNG_ANMELDEN: &str = "geli.kuendigung.anmelden";
/// GNB: confirm an inbound gas Abmeldung (inbound 44004 → UTILMD 44005).
pub const GELI_LIEFERENDE_BESTAETIGEN: &str = "geli.lieferende.bestaetigen";
/// GNB: reject an inbound gas Abmeldung (inbound 44004 → UTILMD 44006).
pub const GELI_LIEFERENDE_ABLEHNEN: &str = "geli.lieferende.ablehnen";
/// LF: initiate a `GeLi` Gas Stornierung (UTILMD 44022/44023).
pub const GELI_STORNIERUNG_INITIIEREN: &str = "geli.stornierung.initiieren";

// ── WiM Strom ─────────────────────────────────────────────────────────────────

/// NB (PID 55042) / MSBA (PID 55039): answer an inbound MSB-Wechsel order
/// positively. The Anmeldung/Kündigung distinction lives in the spawned
/// `wim-geraetewechsel` process (keyed by `MeLo`), not in the command name.
pub const WIM_GERAETEWECHSEL_BESTAETIGEN: &str = "wim.geraetewechsel.bestaetigen";
/// NB (PID 55042) / MSBA (PID 55039): answer an inbound MSB-Wechsel order
/// negatively (APERAK with reason).
pub const WIM_GERAETEWECHSEL_ABLEHNEN: &str = "wim.geraetewechsel.ablehnen";

/// The **technical** acknowledgement on a `WiM` MSB-Wechsel process — 45
/// minutes for Strom `UTILMD`, and not the business answer.
///
/// [`WIM_GERAETEWECHSEL_BESTAETIGEN`] carries that, on its own clock of
/// 3 / 5 / 7 / 1 Werktagen. Two messages, two Fristen, two commands.
pub const WIM_GERAETEWECHSEL_APERAK: &str = "wim.geraetewechsel.aperak";
/// MSB: answer an inbound Steuerungsauftrag positively (ORDRSP).
pub const WIM_STEUERUNGSAUFTRAG_BESTAETIGEN: &str = "wim.steuerungsauftrag.bestaetigen";
/// MSB: answer an inbound Steuerungsauftrag negatively (ORDRSP).
pub const WIM_STEUERUNGSAUFTRAG_ABLEHNEN: &str = "wim.steuerungsauftrag.ablehnen";
/// aMSB: answer an inbound REQOTE Preisanfrage (35001/35002/35004/35005) with the
/// QUOTES Angebot (15001/15002/15004/15005).
pub const WIM_PREISANFRAGE_ANGEBOT_SENDEN: &str = "wim.preisanfrage.angebot-senden";

// ── Cross-check list ──────────────────────────────────────────────────────────

/// Every command name dispatched by out-of-process services.
///
/// `makod` has a registry test asserting each of these is registered; adding a
/// constant above without registering the command in `makod` fails that test.
/// LFN: agree to an announced Zuordnung to an erzeugende Marktlokation or
/// Tranche — inbound 55607, answered UTILMD 55608 (EBDs `E_0603`–`E_0606`).
///
/// The Zustimmung names the Bilanzkreis; without an answer by 15:00 Uhr am ÜT
/// the NB assigns the LFN anyway (GPKE Teil 2 § 2.4.2.2 Nr. 3).
pub const GPKE_ZUORDNUNG_LF_BESTAETIGEN: &str = "gpke.zuordnung-lf.bestaetigen";
/// LFN: refuse an announced Zuordnung — inbound 55607, answered UTILMD 55609.
pub const GPKE_ZUORDNUNG_LF_ABLEHNEN: &str = "gpke.zuordnung-lf.ablehnen";

/// UTILMD 55017 — the LFA agrees to an inbound Kündigung (EBD `E_0614`).
pub const GPKE_KUENDIGUNG_BESTAETIGEN: &str = "gpke.kuendigung.bestaetigen";
/// UTILMD 55018 — the LFA refuses an inbound Kündigung (EBD `E_0614`).
pub const GPKE_KUENDIGUNG_ABLEHNEN: &str = "gpke.kuendigung.ablehnen";

/// UTILMD G 44008 — the LF agrees to an Abmeldung NN vom NB (`E_3002`).
pub const GELI_NB_LIEFERENDE_BESTAETIGEN: &str = "geli.nb-lieferende.bestaetigen";
/// UTILMD G 44009 — the LF refuses an Abmeldung NN vom NB (`E_3002`).
pub const GELI_NB_LIEFERENDE_ABLEHNEN: &str = "geli.nb-lieferende.ablehnen";
/// UTILMD G 44011 — the LFA agrees to an Abmeldeanfrage des NB (`E_3020`).
pub const GELI_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN: &str = "geli.beendigung-zuordnung.bestaetigen";
/// UTILMD G 44012 — the LFA refuses an Abmeldeanfrage des NB (`E_3020`).
pub const GELI_BEENDIGUNG_ZUORDNUNG_ABLEHNEN: &str = "geli.beendigung-zuordnung.ablehnen";
/// UTILMD G 44017 — the LFA agrees to a Gas Kündigung (`E_3001`).
pub const GELI_KUENDIGUNG_BESTAETIGEN: &str = "geli.kuendigung.bestaetigen";
/// UTILMD G 44018 — the LFA refuses a Gas Kündigung (`E_3001`).
pub const GELI_KUENDIGUNG_ABLEHNEN: &str = "geli.kuendigung.ablehnen";
/// UTILMD G 44014 — the E/G agrees to a Gas EoG-Anmeldung (`E_3008`).
pub const GELI_EOG_BESTAETIGEN: &str = "geli.eog.bestaetigen";
/// UTILMD G 44015 — the E/G refuses a Gas EoG-Anmeldung (`E_3008`).
pub const GELI_EOG_ABLEHNEN: &str = "geli.eog.ablehnen";

pub const DISPATCHED_BY_SERVICES: &[&str] = &[
    GPKE_LIEFERBEGINN_ANMELDEN,
    GPKE_EOG_ANMELDEN,
    GPKE_LIEFERBEGINN_BESTAETIGEN,
    GPKE_LIEFERBEGINN_ABLEHNEN,
    GPKE_NEUANLAGE_BESTAETIGEN,
    GPKE_NEUANLAGE_ABLEHNEN,
    GPKE_LIEFERENDE_ANMELDEN,
    GPKE_KUENDIGUNG_ANMELDEN,
    GPKE_LIEFERENDE_BESTAETIGEN,
    GPKE_LIEFERENDE_ABLEHNEN,
    GPKE_NB_LIEFERENDE_BESTAETIGEN,
    GPKE_NB_LIEFERENDE_ABLEHNEN,
    GPKE_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN,
    GPKE_BEENDIGUNG_ZUORDNUNG_ABLEHNEN,
    GPKE_ZUORDNUNG_LF_BESTAETIGEN,
    GPKE_ZUORDNUNG_LF_ABLEHNEN,
    GPKE_KUENDIGUNG_BESTAETIGEN,
    GPKE_KUENDIGUNG_ABLEHNEN,
    GELI_NB_LIEFERENDE_BESTAETIGEN,
    GELI_NB_LIEFERENDE_ABLEHNEN,
    GELI_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN,
    GELI_BEENDIGUNG_ZUORDNUNG_ABLEHNEN,
    GELI_KUENDIGUNG_BESTAETIGEN,
    GELI_KUENDIGUNG_ABLEHNEN,
    GELI_EOG_BESTAETIGEN,
    GELI_EOG_ABLEHNEN,
    GELI_LIEFERBEGINN_ANMELDEN,
    GELI_LIEFERBEGINN_BESTAETIGEN,
    GELI_LIEFERBEGINN_ABLEHNEN,
    GELI_LIEFERENDE_ANMELDEN,
    GELI_KUENDIGUNG_ANMELDEN,
    GELI_LIEFERENDE_BESTAETIGEN,
    GELI_LIEFERENDE_ABLEHNEN,
    GELI_STORNIERUNG_INITIIEREN,
    WIM_GERAETEWECHSEL_BESTAETIGEN,
    WIM_GERAETEWECHSEL_ABLEHNEN,
    WIM_STEUERUNGSAUFTRAG_BESTAETIGEN,
    WIM_STEUERUNGSAUFTRAG_ABLEHNEN,
    WIM_PREISANFRAGE_ANGEBOT_SENDEN,
];
