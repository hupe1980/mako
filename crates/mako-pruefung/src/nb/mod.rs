//! The trees whose **prüfende Rolle** is the Netzbetreiber.
//!
//! | Function | Inbound PIDs | EBD |
//! |---|---|---|
//! | [`evaluate`] | 55001, 55077, 44001 | `E_0622` „Prüfen, ob Anmeldung direkt ablehnbar" |
//! | [`evaluate_abmeldung`] | 55004, 44004 | `E_0607` „Abmeldung prüfen" |
//!
//! They are separate functions because the trees have separate Codelisten:
//! `A02` is „Marktlokation nimmt nicht an der Marktkommunikation teil" in
//! `E_0622` and „Vorlauffrist nicht eingehalten" in `E_0607`. Resolve a code
//! against its tree with [`crate::codes::lookup`].
//!
//! # `evaluate` — the Anmeldung, six deterministic checks
//!
//! Required by GPKE (BK6-24-174, EBD `E_0622`) and GeLi Gas (AWH GeLi Gas 2.0,
//! Codeliste `G_0011`):
//!
//! | # | Rule | Outcome on failure |
//! |---|------|-------------------|
//! | 1 | MaLo exists in NB grid | `Escalate` (data gap) |
//! | 2 | MaLo participates in MaKo (not Stillgelegt/Ruhend) | `Reject(A02)` |
//! | 3 | No conflicting Anmeldung in Bearbeitung | `Reject(A06)` |
//! | 4 | Date plausibility, Transaktionsgrund-aware (Strom: LFW24 future rule, §10c EEG Monatserster for an erzeugende MaLo; Gas: 6-week retro window for E01/E02 SLP, 10 WT for E03) | `Reject(A07)` Strom / `Reject(E17)` Gas / `Escalate` |
//! | 5 | Bilanzierungsgebiet consistent | `Reject(A05)` |
//! | 6 | LF registered in partner directory | `Reject(A05)` |
//!
//! Checks run in order; the first failure short-circuits the rest. `Accept`
//! means every applicable rule passed.

pub mod abmeldung;
pub mod anmeldung;
pub mod config;
pub mod types;

pub use abmeldung::evaluate_abmeldung;
pub use anmeldung::evaluate;
pub use config::NetzCheckConfig;
pub use types::{
    AbmeldungAnfrage, AnmeldungAnfrage, MaloGridRecord, Messtyp, NbEntscheidung, RejectReason,
};
