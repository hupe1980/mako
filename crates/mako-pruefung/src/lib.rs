#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::missing_errors_doc)]
// German regulatory terms (GPKE, GeLi, MaLo, MaStR, BK…) are not Rust items.
#![allow(clippy::doc_markdown)]
//! `mako-pruefung` — decides the **Antwortnachricht** a market participant owes.
//!
//! Every inbound GPKE or GeLi Gas message that expects an answer has published
//! rules for what that answer must be. BDEW prints them as
//! *Entscheidungsbaum-Diagramme und Codelisten für die Antwortnachrichten*
//! (4.3, 01.04.2026): each set of rules names a *prüfende Rolle*, walks
//! numbered Prüfschritte, and lands on a code from **its own** Codeliste. This
//! crate is those rules, executable.
//!
//! The answer rides in `SG4 STS+E01` of the outbound UTILMD — the code in
//! DE 9013, the EBD id in DE 1131 — or, for an invoice, in REMADV `AJT`.
//!
//! | Module | Prüfende Rolle | Trees |
//! |---|---|---|
//! | [`nb`] | Netzbetreiber | `E_0622` Anmeldung, `E_0607` Abmeldung |
//! | [`lf`] | Lieferant | `E_0609`, `E_0624`, `E_0614`, `E_0615`, `E_0603`–`E_0606`; Gas `E_3001`, `E_3002`, `E_3020`, `E_3008` |
//! | [`msb`] | Messstellenbetrieb | `E_0200`–`E_0203` und die WiM-Codelisten |
//! | [`esa`] | MSB (Bestellung) und ESA (Abrechnung) | `E_0252`, `E_0254`, `E_0256`, `E_0257` und `E_0264`–`E_0267` |
//! | [`rechnung`] | ESA, LF, NB | one invoice walk, three [`rechnung::FAMILIEN`] |
//! | [`emob`] | NB (VNB), LF | NZR-EMob / Modell 2: `E_0510`–`E_0513` |
//! | [`mabis`] | NB, LF, BKV | the 28 MaBiS and Redispatch trees (`E_0004`–`E_0104`, `E_0901`, `E_0902`) |
//!
//! The document defines around sixty trees with the LF as prüfende Rolle;
//! these are its **process** answers, the messages that move a Marktlokation
//! between suppliers. Rechnungsprüfung (`E_0406`) and Stammdatenänderung
//! (`E_0408`) are separate obligations — see [`codes::E_0406_CODES`] for what
//! of `E_0406` is resolved here.
//!
//! # MaBiS widens the Cluster axis
//!
//! A GPKE answer is a Zustimmung or an Ablehnung, and the cluster picks
//! between two PIDs. MaBiS needs more, and the differences are observable:
//!
//! - an **Abweisung** was refused *before* it was assessed, and MaBiS
//!   Kap. 9.8.2 Nr. 2 says its Prüfmitteilung is **not forwarded** — while an
//!   Ablehnung's is;
//! - **Ablehnung der gesamten Liste** carries no positions, a
//!   **Korrekturliste wegen Ablehnung** is itself a list of them;
//! - a **Reklamation** tree publishes no Zustimmung at all, because an
//!   acceptable profile is answered with silence.
//!
//! Reducing any of these to `zustimmung: bool` loses a decision the market
//! acts on, so [`Cluster`] names all eight and
//! [`AntwortCode::ist_zustimmung`] answers `None` off the agreement axis.
//!
//! # Why one crate and not one per role
//!
//! **A code has no meaning without its tree.** `A02` is
//!
//! - „Vorlauffrist nicht eingehalten" in `E_0607` (NB answering an Abmeldung),
//! - „Marktlokation nimmt nicht an der Marktkommunikation teil" in `E_0622`
//!   (NB answering an Anmeldung),
//! - „Lieferende zum Abmeldedatum wurde bereits bestätigt" in `E_0609`
//!   (LF answering an Ankündigung),
//!
//! and a combined NB+LF deployment runs all three. One catalogue keyed by EBD
//! is what makes a code checkable at all: [`codes::lookup`] resolves it
//! **within** its tree, and the code's [`Cluster`] — not a caller-supplied
//! boolean — decides whether the answer rides the Bestätigungs- or the
//! Ablehnungs-PID.
//!
//! Role separation stays available where it is load-bearing: the `role-nb`,
//! `role-lf`, `role-msb`, `role-esa`, `role-mabis` and `role-emob` Cargo
//! features compile only their own trees, so a role-gated `processd` binary
//! cannot form a decision it is not entitled to make — the informatorische
//! Entflechtung of § 6a EnWG, enforced at compile time. (§ 7 EnWG, the
//! *rechtliche* Entflechtung, asks for separate legal entities; no Cargo
//! feature satisfies that.)
//!
//! # Antwortcode, not ERC
//!
//! An Antwortcode is **not** an ERC code. `ERC` is the APERAK/CONTRL segment
//! for *processability* errors, with its own catalogue in
//! [`mako_engine::erc`]; an Antwortcode is a business answer and travels in
//! `STS+E01` (UTILMD) or `AJT` (REMADV).
//!
//! [`mako_engine::erc`]: https://docs.rs/mako-engine
//!
//! # Design constraints
//!
//! - **No I/O** — every input is a function argument.
//! - **No clock** — the current instant is passed in, so callers control time.
//! - **Deterministic** — same inputs, same output, always.
//! - **No async.**
//! - **Never guess.** A Prüfschritt the caller's records cannot answer produces
//!   an escalation naming that Prüfschritt, not a plausible code. An answer to
//!   the market is a binding statement about a contract.

pub mod antwort;
pub mod codes;
pub mod error;

pub mod rechnung;

#[cfg(feature = "role-emob")]
pub mod emob;
#[cfg(feature = "role-esa")]
pub mod esa;
#[cfg(feature = "role-lf")]
pub mod lf;
#[cfg(feature = "role-mabis")]
pub mod mabis;
#[cfg(feature = "role-msb")]
pub mod msb;
#[cfg(feature = "role-nb")]
pub mod nb;

pub use antwort::{AntwortDetail, RejectReason};
pub use codes::{AntwortCode, Cluster};
pub use error::CheckError;
pub use mako_fristen::HolidayCalendar;

// The invoice walk is shared by three families and is not ESA-specific — see
// [`rechnung`]. Only the Wertebestellung half below is.
pub use rechnung::{
    PositionsFakten, RechnungsAntwort, RechnungsFakten, RechnungsFamilie, StornoAntwort,
    StornoFakten,
};

#[cfg(feature = "role-esa")]
pub use esa::{
    Bestellart, EsaAnfrage, EsaBeendigung, EsaBestellung, EsaStornierung,
    pruefe_anfrage as pruefe_esa_anfrage, pruefe_beendigung as pruefe_esa_beendigung,
    pruefe_bestellung as pruefe_esa_bestellung, pruefe_rechnung as pruefe_esa_rechnung,
    pruefe_stornierung as pruefe_esa_stornierung,
};

#[cfg(feature = "role-lf")]
pub use lf::{
    Bekannt, Bilanzkreisart, EogZustaendigkeit, LfAnfrage, LfAntwort, LfEntscheidung,
    LfVertragslage, Lokationsart, Terminart, Vollmacht, ZuordnungsFall, ZuordnungsLage,
    pruefe_abmeldung, pruefe_abmeldung_gas, pruefe_abmeldungsanfrage_gas, pruefe_anmeldung_eog,
    pruefe_anmeldung_eog_gas, pruefe_beendigung_zuordnung, pruefe_kuendigung,
    pruefe_kuendigung_gas, pruefe_zuordnung,
};
#[cfg(feature = "role-msb")]
pub use msb::{
    Abmeldegrund, AbmeldungMsb, AnmeldungMsb, Einrichtungsart, KuendigungMsb, Kuendigungstermin,
    MsbEntscheidung, Vertragslage, WeiterverpflichtungAuftrag,
    pruefe_abmeldung as pruefe_abmeldung_msb, pruefe_anmeldung as pruefe_anmeldung_msb,
    pruefe_kuendigung as pruefe_kuendigung_msb, pruefe_weiterverpflichtung,
};
#[cfg(feature = "role-nb")]
pub use nb::{
    Abmeldeanfrage, AbmeldungAnfrage, AnmeldungAnfrage, CODES_REQUIRING_DRITTER, LfaAntwort,
    MaloGridRecord, Messtyp, NbEntscheidung, NetzCheckConfig, TranchenAntwort, TranchenLage,
    TranchenZuordnung, evaluate, evaluate_abmeldung, evaluate_lieferbeginn,
};

// ── Transaktionsgrund codes the trees branch on ───────────────────────────────
//
// Re-exported as crate-level constants because both role modules test the same
// `STS+7` DE 9013 values and a second spelling of `"Z33"` is a silent branch
// that never fires.

/// `E01` — Ein-/Auszug (Umzug).
pub const EIN_AUSZUG: &str = "E01";
/// `E03` — Wechsel.
pub const WECHSEL: &str = "E03";
/// `Z33` — Auszug wegen Stilllegung.
pub const STILLLEGUNG: &str = "Z33";
/// `ZT0` — Abmeldung wegen fehlender Zuordnungsermächtigung aufgrund Änderung ZRT.
pub const ZRT_AENDERUNG: &str = "ZT0";
/// `ZQ7` — Abmeldung wegen fehlender Zuordnungsermächtigung, aufgrund
/// Deaktivierung durch den BKV beim NB.
pub const BKV_DEAKTIVIERUNG: &str = "ZQ7";

/// Every Transaktionsgrund the UTILMD AHB admits on a **55007** Abmeldung.
///
/// `E_0609` reaches Prüfschritt 80 only from 50-nein, and its Hinweis states
/// that the remaining ground is necessarily one of the two
/// Zuordnungsermächtigungs-Codes. A message carrying anything else — or
/// nothing — cannot be walked, so the tree escalates instead of falling
/// through to the Zustimmung.
///
/// `ZC6`/`ZC7` are **not** in this set: those are 55013 Ersatz-/Grundversorgung
/// grounds („EoG aus Bilanzkreisschließung", „EoG aufgrund Erlöschen der
/// Zuordnungsermächtigung"), and reading them here silently confirmed every
/// Abmeldung wegen fehlender Zuordnungsermächtigung.
pub const ABMELDUNG_GRUENDE: &[&str] = &[STILLLEGUNG, BKV_DEAKTIVIERUNG, ZRT_AENDERUNG];
