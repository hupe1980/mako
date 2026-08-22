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
//! | [`lf`] | Lieferant | `E_0609`, `E_0624`, `E_0614`, `E_0615`; Gas `E_3001`, `E_3002`, `E_3020`, `E_3008` |
//!
//! The document defines around sixty trees with the LF as prüfende Rolle;
//! these are its **process** answers, the messages that move a Marktlokation
//! between suppliers. Rechnungsprüfung (`E_0406`), Stammdatenänderung
//! (`E_0408`) and MaBiS (`E_0004`) are separate obligations — see
//! [`codes::E_0406_CODES`] for what of `E_0406` is resolved here.
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
//! Role separation stays available where it is load-bearing (§ 7 EnWG): the
//! `role-nb` and `role-lf` Cargo features compile only their own trees, so a
//! role-gated `processd` binary carries only the decisions it is allowed to make.
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

#[cfg(feature = "role-lf")]
pub mod lf;
#[cfg(feature = "role-msb")]
pub mod msb;
#[cfg(feature = "role-nb")]
pub mod nb;

pub use antwort::{AntwortDetail, RejectReason};
pub use codes::{AntwortCode, Cluster};
pub use error::CheckError;
pub use mako_fristen::HolidayCalendar;

#[cfg(feature = "role-lf")]
pub use lf::{
    Bekannt, EogZustaendigkeit, LfAnfrage, LfAntwort, LfEntscheidung, LfVertragslage, Lokationsart,
    Vollmacht, pruefe_abmeldung, pruefe_abmeldung_gas, pruefe_abmeldungsanfrage_gas,
    pruefe_anmeldung_eog, pruefe_anmeldung_eog_gas, pruefe_beendigung_zuordnung, pruefe_kuendigung,
    pruefe_kuendigung_gas,
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
    AbmeldungAnfrage, AnmeldungAnfrage, MaloGridRecord, Messtyp, NbEntscheidung, NetzCheckConfig,
    evaluate, evaluate_abmeldung,
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
/// `ZC7` — Abmeldung wegen fehlender Zuordnungsermächtigung aufgrund Änderung ZRT.
pub const ZRT_AENDERUNG: &str = "ZC7";
/// `ZC6` — Abmeldung wegen fehlender Zuordnungsermächtigung aufgrund
/// Deaktivierung durch den BKV beim NB.
pub const BKV_DEAKTIVIERUNG: &str = "ZC6";
