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
/// NB: confirm an inbound Lieferbeginn Anmeldung (UTILMD 55003).
pub const GPKE_LIEFERBEGINN_BESTAETIGEN: &str = "gpke.lieferbeginn.bestaetigen";
/// NB: assign a contractless `MaLo` to the Grundversorger (UTILMD 55013, §38 `EnWG`).
pub const GPKE_EOG_ANMELDEN: &str = "gpke.eog.anmelden";
/// NB: reject an inbound Lieferbeginn Anmeldung (UTILMD 55004).
pub const GPKE_LIEFERBEGINN_ABLEHNEN: &str = "gpke.lieferbeginn.ablehnen";
/// LF: initiate a Lieferende Abmeldung (UTILMD 55002).
pub const GPKE_LIEFERENDE_ANMELDEN: &str = "gpke.lieferende.anmelden";
/// LF: confirm an NB-initiated Lieferende (UTILMD 55008 answer).
pub const GPKE_NB_LIEFERENDE_BESTAETIGEN: &str = "gpke.nb-lieferende.bestaetigen";
/// LF: reject an NB-initiated Lieferende.
pub const GPKE_NB_LIEFERENDE_ABLEHNEN: &str = "gpke.nb-lieferende.ablehnen";

// ── GeLi Gas ──────────────────────────────────────────────────────────────────

/// LF: initiate a gas Lieferbeginn Anmeldung (UTILMD 44001).
pub const GELI_LIEFERBEGINN_ANMELDEN: &str = "geli.lieferbeginn.anmelden";
/// NB: confirm an inbound gas Lieferbeginn Anmeldung.
pub const GELI_LIEFERBEGINN_BESTAETIGEN: &str = "geli.lieferbeginn.bestaetigen";
/// NB: reject an inbound gas Lieferbeginn Anmeldung.
pub const GELI_LIEFERBEGINN_ABLEHNEN: &str = "geli.lieferbeginn.ablehnen";
/// LF: initiate a gas Lieferende Abmeldung.
pub const GELI_LIEFERENDE_ANMELDEN: &str = "geli.lieferende.anmelden";
/// LF: initiate a `GeLi` Gas Stornierung (UTILMD 44022/44023).
pub const GELI_GAS_STORNIERUNG_INITIIEREN: &str = "geli.gas.stornierung.initiieren";

// ── WiM Strom ─────────────────────────────────────────────────────────────────

/// NB (PID 55042) / MSBA (PID 55039): answer an inbound MSB-Wechsel order
/// positively. The Anmeldung/Kündigung distinction lives in the spawned
/// `wim-geraetewechsel` process (keyed by `MeLo`), not in the command name.
pub const WIM_GERAETEWECHSEL_BESTAETIGEN: &str = "wim.geraetewechsel.bestaetigen";
/// NB (PID 55042) / MSBA (PID 55039): answer an inbound MSB-Wechsel order
/// negatively (APERAK with reason).
pub const WIM_GERAETEWECHSEL_ABLEHNEN: &str = "wim.geraetewechsel.ablehnen";
/// MSB: answer an inbound Steuerungsauftrag positively (ORDRSP).
pub const WIM_STEUERUNGSAUFTRAG_BESTAETIGEN: &str = "wim.steuerungsauftrag.bestaetigen";
/// MSB: answer an inbound Steuerungsauftrag negatively (ORDRSP).
pub const WIM_STEUERUNGSAUFTRAG_ABLEHNEN: &str = "wim.steuerungsauftrag.ablehnen";
/// aMSB: answer an inbound REQOTE Preisanfrage (35001–35005) with the
/// QUOTES Angebot (15001–15005).
pub const WIM_PREISANFRAGE_ANGEBOT_SENDEN: &str = "wim.preisanfrage.angebot-senden";

// ── Cross-check list ──────────────────────────────────────────────────────────

/// Every command name dispatched by out-of-process services.
///
/// `makod` has a registry test asserting each of these is registered; adding a
/// constant above without registering the command in `makod` fails that test.
pub const DISPATCHED_BY_SERVICES: &[&str] = &[
    GPKE_LIEFERBEGINN_ANMELDEN,
    GPKE_EOG_ANMELDEN,
    GPKE_LIEFERBEGINN_BESTAETIGEN,
    GPKE_LIEFERBEGINN_ABLEHNEN,
    GPKE_LIEFERENDE_ANMELDEN,
    GPKE_NB_LIEFERENDE_BESTAETIGEN,
    GPKE_NB_LIEFERENDE_ABLEHNEN,
    GELI_LIEFERBEGINN_ANMELDEN,
    GELI_LIEFERBEGINN_BESTAETIGEN,
    GELI_LIEFERBEGINN_ABLEHNEN,
    GELI_LIEFERENDE_ANMELDEN,
    GELI_GAS_STORNIERUNG_INITIIEREN,
    WIM_GERAETEWECHSEL_BESTAETIGEN,
    WIM_GERAETEWECHSEL_ABLEHNEN,
    WIM_STEUERUNGSAUFTRAG_BESTAETIGEN,
    WIM_STEUERUNGSAUFTRAG_ABLEHNEN,
    WIM_PREISANFRAGE_ANGEBOT_SENDEN,
];
