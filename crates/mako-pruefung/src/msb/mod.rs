//! The trees whose **prüfende Rolle** is a Messstellenbetreiber — or the
//! Netzbetreiber answering one.
//!
//! | Function | Inbound PID | Answering role | EBD | Frist |
//! |---|---|---|---|---|
//! | [`pruefe_anmeldung`] | 55042 | NB | `E_0201` | 5 WT |
//! | [`pruefe_kuendigung`] | 55039 | **MSBA** | `E_0200` | 3 WT |
//! | [`pruefe_abmeldung`] | 55051 | NB | `E_0202` | 7 WT |
//! | [`pruefe_weiterverpflichtung`] | 17002 | **MSBA** | `E_0203` | 1 WT |
//! | [`esa::pruefe_bestellung`] | 17007 | MSB | `E_0256` | 2 WT |
//! | [`esa::pruefe_stornierung`] | 39002 | MSB | `E_0257` | 2 WT |
//! | [`esa::pruefe_beendigung`] | 17008 | MSB | `E_0254` | 2 WT |
//!
//! The ESA trees ([`esa`]) answer with an **ORDRSP**, so their code rides
//! `SG2 AJT` (DE 4465 code, DE 1082 tree) rather than `STS+E01`.
//!
//! The Verpflichtungsanfrage (55168 → `E_0240`) is deliberately absent: WiM
//! Teil 1 Kap. 2.4.2 Nr. 4 leaves the answer to the gMSB's own commercial
//! judgement („nach eigenem Ermessen"), so there is no Prüfschritt to execute
//! and nothing here could decide it without inventing a rule. It escalates at
//! the caller.
//!
//! # The directions are not uniform
//!
//! - **55039 never reaches the NB.** It is MSBN → MSBA on the contract layer
//!   (Kap. 2.1.3), so [`pruefe_kuendigung`] asks about the MSBA's own
//!   Messstellenbetriebsvertrag — a wettbewerblicher MSB has no grid registry
//!   to consult.
//! - **17002 is the NB ordering the MSBA to stay**, and its answer is an
//!   ORDRSP, not a UTILMD.
//! - **55051 is MSBA → NB**, so the abmeldender MSB is the sender.
//!
//! # Vorlauffristen
//!
//! Kap. 2.3.2 Nr. 2 gives the NB three duties on an Anmeldung:
//!
//! 1. Vorliegen der Versicherung über die Beauftragung → `ZB6`
//! 2. **Zulässiger Zuordnungsbeginn: Einhaltung der Mindestvorlaufzeit** → `E17`
//! 3. Vorliegen eines Vertrages nach § 9 Abs. 1 Nr. 3 MsbG mit dem MSBN
//!
//! The Mindestvorlaufzeit is 15 Werktage, or 7 at erstmaliger Einrichtung
//! ([`mako_fristen::vorlauf`]). Without it every Anmeldung is confirmed on the
//! date the counterparty picked, and the Realisierungskorridor of ±9 Werktagen
//! around that date extends into a window the NB cannot serve.
//!
//! The Abmeldung's own 20-Werktage rule is **not** a rejection: Kap. 2.4.2
//! Nr. 2 has the NB move the Zuordnungsende to the nächstmögliches and confirm
//! with `Z01`. `E_0202` publishes `E17` only for the Aufhebung einer
//! zukünftigen Zuordnung.
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2a, WiM Strom Teil 1 Kap. 2.2–2.4
//! - Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 8
//! - MsbG §§ 5, 9, 14 (freie Wahl des MSB, Rahmenvertrag, Wechselrecht)

pub mod abmeldung;
pub mod anmeldung;
pub mod esa;
pub mod kuendigung;
pub mod types;
pub mod weiterverpflichtung;

pub use abmeldung::pruefe_abmeldung;
pub use anmeldung::pruefe_anmeldung;
pub use esa::{
    Bestellart, EsaBeendigung, EsaBestellung, EsaStornierung, ebd_fuer_antwort, pruefe_beendigung,
    pruefe_bestellung, pruefe_stornierung,
};
pub use kuendigung::pruefe_kuendigung;
pub use types::{
    Abmeldegrund, AbmeldungMsb, AnmeldungMsb, Einrichtungsart, KuendigungMsb, Kuendigungstermin,
    MsbEntscheidung, Vertragslage, WeiterverpflichtungAuftrag,
};
pub use weiterverpflichtung::pruefe_weiterverpflichtung;

// ── Sparte → Entscheidungsbaum ───────────────────────────────────────────────

/// The Entscheidungsbaum a WiM MSB-Wechsel answer resolves against, per
/// Prozessschritt and Sparte.
///
/// The Prüfschritte are the same in both Sparten; the published alphabets are
/// not. `mako_wim::geraetewechsel::wim_ebd` gives the same answer keyed on the
/// Prüfidentifikator, for callers that have one.
pub mod baum {
    use super::types::Sparte;
    use crate::codes as c;

    /// Kündigung Messstellenbetrieb — `E_0200` / `E_2000`.
    #[must_use]
    pub const fn kuendigung(sparte: Sparte) -> &'static str {
        match sparte {
            Sparte::Strom => c::EBD_KUENDIGUNG_MSB,
            Sparte::Gas => c::EBD_KUENDIGUNG_MSB_GAS,
        }
    }

    /// Anmeldung Messstellenbetrieb — `E_0201` / `E_2002`.
    #[must_use]
    pub const fn anmeldung(sparte: Sparte) -> &'static str {
        match sparte {
            Sparte::Strom => c::EBD_ANMELDUNG_MSB,
            Sparte::Gas => c::EBD_ANMELDUNG_MSB_GAS,
        }
    }

    /// Abmeldung (Ende Messstellenbetrieb) — `E_0202` / `E_2005`.
    #[must_use]
    pub const fn abmeldung(sparte: Sparte) -> &'static str {
        match sparte {
            Sparte::Strom => c::EBD_ABMELDUNG_MSB,
            Sparte::Gas => c::EBD_ABMELDUNG_MSB_GAS,
        }
    }

    /// Weiterverpflichtung des MSB — `E_0203` / `E_2004`.
    #[must_use]
    pub const fn weiterverpflichtung(sparte: Sparte) -> &'static str {
        match sparte {
            Sparte::Strom => c::EBD_WEITERVERPFLICHTUNG,
            Sparte::Gas => c::EBD_WEITERVERPFLICHTUNG_GAS,
        }
    }
}
