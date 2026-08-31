//! `mako-emob` — **NZR-EMob / Modell 2**: the virtual Bilanzierungsgebiet that
//! lets a driver bring their own supplier to a charge point.
//!
//! A Ladepunktbetreiber (LPB/CPO) runs a regelzonenweites Bilanzierungsgebiet of
//! its own, and every registered Übergabestelle's flows are an exchange between
//! the VNB's BG and that one (Anlage 6 zum Beschluss **BK6-20-160**). The
//! Marktlokation is then balanced in the LPB's BG — „**Modell 2**" — and the LPB
//! assigns each charging session's energy to the Bilanzkreis of the supplier the
//! customer chose, quarter hour by quarter hour, with intraday supplier changes.
//! **BK6-24-267** extends the model to a private Kundenanlage under § 20 Abs. 1,
//! 1a EnWG.
//!
//! This crate is the **allocation engine, its invariants, and the three UTILMD
//! legs that move a Marktlokation between the models** — pure domain: no I/O,
//! no wire rendering, no persistence.
//!
//! | Module | Answers |
//! |---|---|
//! | [`bg`] | may this Bilanzierungsgebiet exist, here, now? |
//! | [`uebergabestelle`] | may this Marktlokation enter Modell 2? |
//! | [`session`] | what quarter-hour energies does this Ladevorgang produce? |
//! | [`allocation`] | who gets this quarter hour, and what is left over? |
//! | [`fristen`] | when is each step due? |
//! | [`modellwechsel`] | which UTILMD leg moves it, and what does the answer carry? |
//! | [`wire`] | which UTILMD qualifier carries it? |
//! | [`ids`] | which identifiers are ours to mint, and which are not? |
//!
//! The Entscheidungsbäume (`E_0510`–`E_0513`) live in `mako_pruefung::emob` and
//! the answer Fristen in `mako_fristen::antwort`, where every other market
//! process keeps them.
//!
//! # The one invariant everything else serves
//!
//! Anlage 6 §IV.1 obliges the LPB to assign **the whole Bilanzierungsgebiet**,
//! every quarter hour:
//!
//! ```text
//! NGZ(t, richtung) = Σ zugeordnete Marktlokationen + Deltamenge
//! ```
//!
//! [`allocation::QuarterHourAllocation`] holds that exactly and returns a
//! [`allocation::ConservationProof`] beside every row. The Deltamenge is a
//! quantity, not a rounding error: it settles in a Bilanzkreis the LPB names, at
//! the LPB's own cost (§IV.2).
//!
//! ```rust
//! use mako_emob::allocation::{Anspruch, MaloKind, QuarterHourAllocation, Richtung};
//! use mako_emob::ids::VirtualMaloId;
//! use mako_emob::session::Viertelstunde;
//! use rust_decimal::dec;
//! use time::macros::datetime;
//!
//! let slot = Viertelstunde::containing(datetime!(2026-11-03 08:07:00 UTC));
//!
//! // 12 kWh crossed the Übergabestelle; two vehicles claim 9 of them.
//! let row = QuarterHourAllocation::allocate(
//!     slot,
//!     Richtung::Bezug,
//!     dec!(12),
//!     &[
//!         Anspruch { malo: VirtualMaloId::new("veh-1")?, kind: MaloKind::Vehicle, kwh: dec!(6) },
//!         Anspruch { malo: VirtualMaloId::new("veh-2")?, kind: MaloKind::Vehicle, kwh: dec!(3) },
//!     ],
//! )?;
//!
//! assert_eq!(row.delta_kwh, dec!(3));   // the LPB's own Bilanzkreis carries these
//! assert!(row.proof.haelt());           // Anlage 6 §IV.1, checked
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # The market role is NB
//!
//! The BDEW Rollenmodell defines no LPB: „der LPB kommuniziert aus prozessualer
//! Sicht wie die Rolle NB" (AWH Kap. 1.4). Every message leaves as **NB**;
//! `mako_engine::marktrolle::Marktrolle::Lpb` is a *deployment* identity, the
//! `Nmsb`/`Amsb` pattern, without which the shared Prüfidentifikatoren are
//! ambiguous in a deployment that is both VNB and LPB.
//!
//! # Prüfidentifikatoren
//!
//! | PID | Message | From → To | Answered by |
//! |---|---|---|---|
//! | 55238 | Anmeldung in Modell 2 | NB (LPB) → NB (VNB) | 55239, `E_0513`→`E_0510` |
//! | 55240 | Beendigung der Zuordnung zur MaLo | NB (VNB) → LF | 55241, `E_0511` |
//! | 55242 | Abmeldung aus dem Modell 2 | NB (LPB) → NB (VNB) | 55243, `E_0512` |
//! | 55235 / 55236 | Zuordnung / Beendigung ZP der NGZ zur NZR | verantw. NB → benachb. NB, ÜNB | 55237, `E_0102` / `E_0103` |
//! | 55062 / 55063 | MaBiS-ZP für die tägliche BK-SZR eMob | NB (LPB) ↔ ÜNB | 55064 |
//! | 13018 | MSCONS Netzgangzeitreihe | NB (VNB) → NB (LPB), ÜNB | — |
//! | 13003 | MSCONS NZR (eMob), BK-SZR (Kat. A) eMob, tägliche BK-SZR eMob | NB ↔ NB, NB → BKV/ÜNB | 21001 / 21002 / 21004 |
//!
//! # Sources
//!
//! - **BK6-20-160** (21.12.2020) Anlage 6 „NZR-EMob"; Mitteilung Nr. 4 (03.05.2022)
//! - **BDEW AWH „Zum Modell 2 zur ladevorgangscharfen bilanziellen
//!   Energiemengenzuordnungsmöglichkeit" V1.3** (01.04.2025)
//! - **BDEW AWH Ergänzung der Marktregeln … Bilanzkreisabrechnung (MaBiS)**
//!   V1.0 (27.04.2022) — the Netzgangzeitreihe
//! - **BK6-24-267** (15.05.2025), bestandskräftig
//! - **UTILMD AHB Strom 2.2** Kap. 11; **EBD 4.3** Kap. 17
//! - **MaBiS** BK6-24-174 Anlage 3, Kap. 3.8 / 3.10 / 5

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic, clippy::must_use_candidate)]
// German regulatory terms (MaLo, MaBiS, NGZ, Bilanzierungsgebiet…) are not Rust items.
#![allow(clippy::doc_markdown)]

pub mod allocation;
pub mod bg;
pub mod error;
pub mod fristen;
pub mod ids;
pub mod modellwechsel;
pub mod session;
pub mod uebergabestelle;
pub mod wire;

pub use allocation::{
    AllocationVersion, Anspruch, ConservationProof, Datenstatus, MaloKind, QuarterHourAllocation,
    Richtung, Ueberdeckung, Versionsreihe, Zuordnung,
};
pub use bg::{BgRegistry, Regelzone, VirtualBalancingArea};
pub use error::EmobError;
pub use ids::{SessionId, TokenRef, VirtualMaloId};
pub use modellwechsel::{
    EmobAbmeldungWorkflow, EmobAnmeldungWorkflow, EmobAntwort, EmobZuordnungsendeWorkflow, LegWire,
    Modellwechseldaten,
};
pub use session::{Ladevorgang, Provenance, SessionSplit, SlotEnergie, Viertelstunde};
pub use uebergabestelle::{
    Abwicklungsmodell, AccessBasis, MeteringMode, Modellwechsel, Uebergabestelle,
};

/// The Prüfidentifikatoren of the Modellwechsel, in process order.
///
/// `makod` registers these; `mako_pruefung::emob` decides the answers to the
/// three that carry one.
pub const MODELLWECHSEL_PIDS: [u32; 6] = [55_238, 55_239, 55_240, 55_241, 55_242, 55_243];

/// The Prüfidentifikatoren of the Zuordnung des ZP der NGZ zur NZR.
///
/// MaBiS rather than Modell 2 — they come from the AWH Ergänzung der Marktregeln
/// (27.04.2022) and are answered from `mako_pruefung::mabis` with `E_0102` and
/// `E_0103`.
pub const ZP_NGZ_PIDS: [u32; 3] = [55_235, 55_236, 55_237];

// ── EngineModule ──────────────────────────────────────────────────────────────

/// Engine module for **NZR-EMob / Modell 2**.
///
/// | Workflow | PIDs |
/// |---|---|
/// | `emob-anmeldung` | UTILMD 55238 / 55239 |
/// | `emob-zuordnungsende` | UTILMD 55240 / 55241 |
/// | `emob-abmeldung` | UTILMD 55242 / 55243 |
///
/// # What this module does not own
///
/// 55235–55237 (Zuordnung des ZP der NGZ zur NZR) are **MaBiS**, registered by
/// `mako_mabis::MabisModule` and answered from `mako_pruefung::mabis` with
/// `E_0102` / `E_0103`. MSCONS 13018 (Netzgangzeitreihe) and 13003 (NZR and
/// the BK-SZR eMob) are MaBiS Summenzeitreihen and belong to `mabis-billing`.
/// Registering any of them here would take them off a settlement stream that
/// already handles them.
pub struct EmobModule;

impl mako_engine::builder::EngineModule for EmobModule {
    fn name(&self) -> &'static str {
        "emob"
    }

    fn workflow_names(&self) -> &'static [&'static str] {
        &[
            modellwechsel::EmobAnmeldungWorkflow::WORKFLOW_NAME,
            modellwechsel::EmobZuordnungsendeWorkflow::WORKFLOW_NAME,
            modellwechsel::EmobAbmeldungWorkflow::WORKFLOW_NAME,
        ]
    }

    fn register_pids(&self, router: &mut mako_engine::pid_router::PidRouter) {
        // Both PIDs of a leg route to the same workflow: the answer arrives on
        // a process this side started and resumes it. Routing an answer PID
        // nowhere would dead-letter every reply and let the Frist expire as a
        // false timeout.
        for leg in [
            modellwechsel::ANMELDUNG,
            modellwechsel::ZUORDNUNGSENDE,
            modellwechsel::ABMELDUNG,
        ] {
            let name = match leg.anfrage_pid {
                55_238 => modellwechsel::EmobAnmeldungWorkflow::WORKFLOW_NAME,
                55_240 => modellwechsel::EmobZuordnungsendeWorkflow::WORKFLOW_NAME,
                _ => modellwechsel::EmobAbmeldungWorkflow::WORKFLOW_NAME,
            };
            router.register(leg.anfrage_pid, name);
            router.register(leg.antwort_pid, name);
        }
    }

    fn profile_requirements(&self) -> &'static [mako_engine::profile::ProfileRequirement] {
        &[mako_engine::profile::ProfileRequirement {
            message_type: "UTILMD",
            label: "UTILMD Strom (NZR-EMob / Modell 2, AHB Kap. 11)",
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_pid_families_are_disjoint() {
        for pid in ZP_NGZ_PIDS {
            assert!(
                !MODELLWECHSEL_PIDS.contains(&pid),
                "{pid} belongs to one family only"
            );
        }
    }

    /// Every PID the module routes is one of the six the crate publishes, and
    /// every one of the six is routed — an unrouted PID dead-letters.
    #[test]
    fn the_module_routes_exactly_the_modellwechsel_pids() {
        use mako_engine::builder::EngineModule;
        let mut router = mako_engine::pid_router::PidRouter::new();
        EmobModule.register_pids(&mut router);
        for pid in MODELLWECHSEL_PIDS {
            assert!(
                router.route(pid).is_some(),
                "{pid} routes nowhere and would dead-letter"
            );
        }
        // 55235–55237 stay with MaBiS.
        for pid in ZP_NGZ_PIDS {
            assert!(router.route(pid).is_none(), "{pid} belongs to mabis");
        }
    }

    /// Both PIDs of a leg share a workflow, and no two legs share one.
    #[test]
    fn each_leg_owns_one_workflow() {
        use mako_engine::builder::EngineModule;
        let mut router = mako_engine::pid_router::PidRouter::new();
        EmobModule.register_pids(&mut router);
        for (anfrage, antwort) in [(55_238, 55_239), (55_240, 55_241), (55_242, 55_243)] {
            assert_eq!(router.route(anfrage), router.route(antwort));
        }
        let mut names: Vec<_> = EmobModule.workflow_names().to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 3);
    }

    #[test]
    fn every_modellwechsel_pid_is_a_utilmd_strom_pid() {
        assert!(
            MODELLWECHSEL_PIDS
                .iter()
                .all(|p| (55_000..56_000).contains(p))
        );
        assert!(ZP_NGZ_PIDS.iter().all(|p| (55_000..56_000).contains(p)));
    }
}
