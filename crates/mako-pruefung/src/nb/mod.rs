//! The trees whose **prüfende Rolle** is the Netzbetreiber.
//!
//! | Function | Inbound PIDs | EBD |
//! |---|---|---|
//! | [`evaluate`] | 55001, 55077, 44001 | `E_0622` / `E_3005` „Prüfen, ob Anmeldung direkt ablehnbar" |
//! | [`evaluate_abmeldung`] | 55004, 44004 | `E_0607` / `E_3019` „Abmeldung prüfen" |
//! | [`evaluate_neuanlage`] | 55600, 55601 | `E_0608` „Anmeldung einer Zuordnung" |
//!
//! They are separate functions because the trees have separate Codelisten:
//! `A02` is „Marktlokation nimmt nicht an der Marktkommunikation teil" in
//! `E_0622` and „Vorlauffrist nicht eingehalten" in `E_0607`. Resolve a code
//! against its tree with [`crate::codes::lookup`].
//!
//! # `evaluate` — the Anmeldung
//!
//! `E_0622` Prüfschritt 10 splits Strom into two branches that share no code,
//! and Gas answers from `G_0011`:
//!
//! | Condition | verbrauchend/ruhend | erzeugend | Gas |
//! |---|---|---|---|
//! | Vorlauffrist | `A07` | `A34`/`A28`/`A29`/`A30`/`A32`/`A35`/`A44` | `E17` |
//! | nimmt nicht an der MaKo teil | `A02` | — | `A16` |
//! | Zuordnungsermächtigung | `A05` | `A25` | `E13` |
//! | andere Anmeldung in Bearbeitung | `A06` | `A45` | `ZC5` |
//! | Zustimmung | `A51` (`E_0623`) | `A58` (`E_0623`) | `E15` (`G_0012`) |
//!
//! `E_0622` and `E_3005` are **Vorprüfungen** — every code they publish is an
//! Ablehnung, and a surviving message is confirmed out of `E_0623` / `E_3007`.
//! `SG4 STS+E01` is Muss on every Antwortnachricht, so `Accept` carries a code.
//!
//! # `evaluate_neuanlage` — the Neuanlage, and its third outcome
//!
//! `E_0608` Prüfschritte 110 / 590 are a **loop**: an Anmeldung whose
//! Marktlokation cannot yet be identified is re-checked daily for 60 Werktage
//! before it may be refused. [`NeuanlageEntscheidung::Vertagen`] is that state —
//! the NB answers nothing at all that day.
//!
pub mod abmeldung;
pub mod anmeldung;
pub mod config;
pub mod lieferbeginn;
pub mod neuanlage;
pub mod types;

pub use abmeldung::evaluate_abmeldung;
pub use anmeldung::evaluate;
pub use config::NetzCheckConfig;
pub use lieferbeginn::{CODES_REQUIRING_DRITTER, evaluate_lieferbeginn};
pub use neuanlage::{
    Identifikation, NeuanlageAnfrage, NeuanlageBefund, NeuanlageEntscheidung, evaluate_neuanlage,
};
pub use types::{
    Abmeldeanfrage, AbmeldungAnfrage, AnmeldungAnfrage, AntwortDetail, ErzeugungsAnmeldung,
    Geschaeftsvorfall, LfaAntwort, MaloGridRecord, Marktlokationsart, Messtyp, NbEntscheidung,
    RejectReason, TranchenAntwort, TranchenLage, TranchenZuordnung, Veraeusserungsform,
};
