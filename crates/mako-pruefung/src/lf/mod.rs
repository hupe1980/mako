//! The trees whose **prüfende Rolle** is the Lieferant.
//!
//! | Module | Process | Strom | Gas |
//! |---|---|---|---|
//! | [`abmeldung`] | Lieferende von NB an LF | 55007 → `E_0609` | 44007 → `E_3002` |
//! | [`beendigung_zuordnung`] | Abmeldeanfrage im Lieferbeginn | 55010 → `E_0624` | 44010 → `E_3020` |
//! | [`kuendigung`] | Kündigung beim Altlieferanten | 55016 → `E_0614` | 44016 → `E_3001` |
//! | [`eog`] | Anmeldung E/G (§ 36 / § 38 EnWG) | 55013 → `E_0615` | 44013 → `E_3008` |
//! | [`zuordnung`] | Ankündigung Zuordnung LF (erz. MaLo / Tranche) | 55607 → `E_0603`–`E_0606` | — |
//!
//! Split by **process**, not by Sparte, matching [`crate::nb`]: the Strom tree
//! and the Gas Codeliste for one business process are counterparts.
//!
//! [`eog`] is the supplier's only Anmeldung tree. A supplier *sends* the
//! ordinary Anmeldung and the Netzbetreiber checks it (`E_0622`, in
//! [`crate::nb`]); the one it is asked to check is the one the NB assigns to
//! it. Neuanlage is NB-side too — `E_0608` names the Netzbetreiber.
//!
//! # Two invariants
//!
//! **Never guess.** A Prüfschritt the caller's records cannot answer produces
//! [`LfEntscheidung::Eskalation`] naming it, never a plausible code.
//! [`Bekannt::Unbekannt`] carries "no record either way" into the walk;
//! collapsing it to `false` makes a supplier agree to release a customer it
//! still holds under contract.
//!
//! **The Cluster picks the PID.** Every code sits in a published Zustimmungs-
//! or Ablehnungs-Cluster, reported by [`LfAntwort::zustimmung`]. A separate
//! `accepted: bool` could disagree with the code and send `A35` „Es besteht
//! eine Vertragsbindung" as a Bestätigung.

pub mod abmeldung;
pub mod beendigung_zuordnung;
pub mod eog;
pub mod kuendigung;
pub mod types;
pub mod zuordnung;

pub use abmeldung::{pruefe_abmeldung, pruefe_abmeldung_gas};
pub use beendigung_zuordnung::{pruefe_abmeldungsanfrage_gas, pruefe_beendigung_zuordnung};
pub use eog::{EogZustaendigkeit, pruefe_anmeldung_eog, pruefe_anmeldung_eog_gas};
pub use kuendigung::{pruefe_kuendigung, pruefe_kuendigung_gas};
pub use types::{
    Bekannt, LfAnfrage, LfAntwort, LfEntscheidung, LfVertragslage, Lokationsart, Terminart,
    Vollmacht,
};
pub use zuordnung::{Bilanzkreisart, ZuordnungsFall, ZuordnungsLage, pruefe_zuordnung};
