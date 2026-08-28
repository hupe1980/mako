//! Every tree of the **Energieserviceanbieter** relationship — WiM Strom
//! Teil 2, Kapitel 4.
//!
//! The ESA is the consent-derived role that orders and receives Messwerte from
//! an MSB on behalf of an Anschlussnutzer (§49 Abs. 2 Nr. 9 MsbG). Its market
//! life is two halves, and they check in opposite directions:
//!
//! | Half | Module | Prüfende Rolle | Trees |
//! |---|---|---|---|
//! | Ordering values (Kap. 4.1–4.4) | [`wertebestellung`] | **MSB** | `E_0252`, `E_0256`, `E_0257`, `E_0254` |
//! | Paying for them (Kap. 4.5) | [`rechnung`] | **ESA** (and `E_0265` the MSB) | `E_0264`, `E_0265`, `E_0266`, `E_0267` |
//!
//! They live in one module because they are one relationship and share one
//! vocabulary — the Abonnement mode that picks the ordering answer's tree also
//! decides what the invoice may bill — but they are not variations of each
//! other: an ordering answer is one `SG2 AJT` on an ORDRSP, while `E_0264`
//! answers with a **set** of (Ebene, Positionsnummer, code) triples on a
//! REMADV.
//!
//! # Sources
//!
//! - BK6-22-024 Anlage 2b, WiM Strom Teil 2 Kap. 4
//! - *Entscheidungsbaum-Diagramme und Codelisten* 4.3, Kap. 8.25–8.27
//! - ORDRSP AHB 1.1b §4.15, REMADV AHB 1.0a §3, COMDIS AHB 1.0h

pub mod rechnung;
pub mod wertebestellung;

// The Kap. 4.5 billing round trip is one instantiation of the shared invoice
// walk (`crate::rechnung`), not a family of its own — the AWH Preisblatt-B
// chapters publish the same Prüfschritte under twelve further EBD numbers. The
// types are re-exported here so an ESA caller reaches them where the rest of
// the relationship lives.
pub use crate::rechnung::{
    Befund, Ebene, NichtZahlungsavisAntwort, PositionsFakten, RechnungsAntwort, RechnungsFakten,
    Steuersatzpruefung, StornoAntwort, StornoFakten, UrsprungsAntwort, Zeitraum,
};
pub use rechnung::{
    pruefe_nicht_zahlungsavis, pruefe_rechnung, pruefe_rechnung_erneut, pruefe_stornorechnung,
};
pub use wertebestellung::{
    Bestellart, EsaAnfrage, EsaBeendigung, EsaBestellung, EsaStornierung, ebd_fuer_antwort,
    pruefe_anfrage, pruefe_beendigung, pruefe_bestellung, pruefe_stornierung,
};
