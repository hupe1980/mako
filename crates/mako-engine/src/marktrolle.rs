//! BDEW Rollenmodell — market-participant role configuration.
//!
//! The BDEW Rollenmodell für die Marktkommunikation (V2.2, January 2026) explicitly
//! permits a single legal entity to hold multiple market roles simultaneously.
//! Common combinations:
//!
//! | Combination | Regulatory basis |
//! |---|---|
//! | NB + gMSB | §41 MsbG — NB is grundzuständiger MSB for basic meters |
//! | NB + BKV | Stadtwerke managing their own balance group |
//! | NB + LF | Vertically integrated utility |
//! | LF + BKV | Supplier managing its own balance group |
//!
//! ## Why role-awareness matters for PID routing
//!
//! Several EDIFACT PIDs are **shared across process families** and their correct
//! inbound destination depends on which role this `makod` instance fills:
//!
//! | PID | ORDRSP semantics |
//! |---|---|
//! | 19001 (Bestellbestätigung) | → `gpke-konfiguration` when NB receiving from MSB |
//! | 19001 (Bestellbestätigung) | → `wim-geraeteubernahme` when nMSB receiving from NB |
//! | 19015 (Bestätigung Gerätewechselabsicht) | → `wim-geraeteubernahme` when NB receiving from nMSB |
//! | 13003 (MSCONS Summenzeitreihe) | → `mabis-billing` when BKV receiving from BIKO |
//! | 13003 (MSCONS Summenzeitreihe) | → MaBiS NZR handler when NB receiving from NB |
//!
//! By declaring which roles a `makod` instance serves, the engine can register
//! only the PID routes that apply, preventing both silent dead-letters and
//! accidental misrouting.
//!
//! ## Conflict guard
//!
//! [`PidRouter`] panics at build time if two modules register the same PID to
//! **different** workflow names. Set explicit [`DeploymentRoles`] to exclude
//! conflicting registrations from modules that don't apply to this instance.
//!
//! [`PidRouter`]: crate::pid_router::PidRouter

use std::collections::HashSet;

// ── Marktrolle ────────────────────────────────────────────────────────────────

/// A BDEW market-participant role (Marktrolle).
///
/// Declares which roles this `makod` deployment fills within the German energy
/// market communication (MaKo) ecosystem. A single deployment may hold several
/// roles simultaneously (see module-level docs).
///
/// # Non-exhaustive
///
/// New roles may be added as BDEW regulations expand. Match with `_` in
/// exhaustive arms or use [`DeploymentRoles::contains`] for membership checks.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Marktrolle {
    /// Netzbetreiber (NB) — distribution/transmission network operator.
    ///
    /// Receives GPKE ANFRAGE messages (55001/55002/55017), issues ANTWORT
    /// messages (55003–55006), runs GPKE Konfiguration (17134/17135 outbound
    /// ORDERS, 19001/19002 inbound ORDRSP).
    Nb,

    /// Lieferant (LF) — energy supplier.
    ///
    /// Initiates GPKE Lieferbeginn/Lieferende, receives ANTWORT from NB.
    /// Registers as inbound-ANTWORT recipient (55003–55006/55018) for the
    /// LF-side anmeldung workflow.
    Lf,

    /// grundzuständiger Messstellenbetreiber (gMSB) — incumbent meter operator.
    ///
    /// Receives WiM UTILMD device-change messages (11001–11003). Often the same
    /// legal entity as the NB (§41 MsbG).
    Msb,

    /// nicht-grundzuständiger Messstellenbetreiber (nMSB) — challenger meter operator.
    ///
    /// Sends WiM UTILMD device-change requests (11001) and WiM Geräteübernahme
    /// ORDERS (17001, 17009). Receives inbound ORDRSP responses 19001/19002
    /// (Bestellbestätigung/Ablehnung) and 19015/19016 (Gerätewechselabsicht).
    Nmsb,

    /// abgebender Messstellenbetreiber (aMSB) — outgoing meter operator.
    ///
    /// Receives WiM Abmeldung/Kündigung UTILMD (11002). This role is often
    /// held by the gMSB after a successful nMSB takeover.
    Amsb,

    /// Bilanzkreisverantwortlicher (BKV) — balance responsible party.
    ///
    /// Receives MABIS billing MSCONS (PID 13003 from BIKO: Abrechnungssummenzeitreihe).
    Bkv,

    /// Übertragungsnetzbetreiber (ÜNB) — transmission system operator.
    ///
    /// Issues BG-SZR Kategorie B/C and BK-SZR Kategorie B/C MSCONS (PID 13003).
    Uenb,

    /// Bilanzkoordinator (BIKO) — balancing coordinator.
    ///
    /// Issues Abrechnungssummenzeitreihe MSCONS (PID 13003) to BKV and NB-DZR.
    Biko,

    /// Energieserviceanbieter (ESA) — energy service provider acting for the
    /// Anschlussnutzer (PARTIN 37006, "Kommunikationsdaten des ESA Strom").
    ///
    /// **Strom only.** An ESA has no Zuordnung to a Marktlokation: its access to
    /// values rests on the Anschlussnutzer's consent (§49 Abs. 2 Nr. 9 MsbG) and
    /// a bilateral contract with the MSB, which §34 Abs. 2 S. 2 Nr. 10 MsbG makes
    /// a mandatory, non-discriminatory Zusatzleistung.
    ///
    /// Sends REQOTE Anfrage, ORDERS 17007 (Bestellung/Abbestellung) and
    /// ORDCHG 39002 (Stornierung); receives QUOTES 15003 and
    /// ORDRSP 19011/19012/19013/19014, plus the values themselves.
    ///
    /// This role is for a deployment that **is** an ESA. An MSB *serving* an ESA
    /// registers the inbound side under [`Marktrolle::Msb`].
    Esa,
}

// ── DeploymentRoles ───────────────────────────────────────────────────────────

/// The set of [`Marktrolle`]s this `makod` deployment fills.
///
/// Used by [`EngineModule::register_pids_with_roles`] to conditionally register
/// PID routes based on which roles are active. Modules check
/// `roles.contains(Marktrolle::Nb)` before registering role-specific PIDs.
///
/// # Constructors
///
/// - [`DeploymentRoles::all()`] — registers everything regardless of role
///   (useful for development and single-role deployments, default).
/// - [`DeploymentRoles::from_roles`] — explicit set for multi-role conflict resolution.
/// - Convenience methods: [`nb()`], [`lf()`], [`msb()`], [`nmsb()`] etc.
///
/// # Conflict guard
///
/// When two modules both register the same PID to **different** workflow names,
/// `EngineBuilder::build` will detect the conflict and panic. Set exclusive roles
/// to ensure only one workflow is registered per shared PID:
///
/// ```rust,ignore
/// // NB deployment: GPKE registers 19001/19002 → gpke-konfiguration
/// // nMSB deployment: WiM registers 19001/19002 → wim-geraeteubernahme
/// // Combined (conflict!): set roles to prevent double-registration:
/// use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
///
/// let roles = DeploymentRoles::from_roles([Marktrolle::Nb]);
/// // Now only GPKE registers 19001/19002; WiM skips its nMSB-conditional block.
/// ```
///
/// [`EngineModule::register_pids_with_roles`]: crate::builder::EngineModule::register_pids_with_roles
/// [`nb()`]: DeploymentRoles::nb
/// [`lf()`]: DeploymentRoles::lf
/// [`msb()`]: DeploymentRoles::msb
/// [`nmsb()`]: DeploymentRoles::nmsb
#[derive(Debug, Clone)]
pub struct DeploymentRoles {
    /// When `true`, `contains()` returns `true` for every role (matches all).
    all: bool,
    roles: HashSet<Marktrolle>,
}

impl Default for DeploymentRoles {
    /// Defaults to `all` — every role is considered active.
    ///
    /// This preserves backward-compatible behavior (all PIDs registered) for
    /// deployments that have not yet configured explicit roles. Set explicit
    /// roles via [`DeploymentRoles::from_roles`] for multi-role conflict safety.
    fn default() -> Self {
        Self::all()
    }
}

impl DeploymentRoles {
    /// All roles active — `contains` always returns `true`.
    ///
    /// The default for `EngineBuilder`. Modules register all their PIDs
    /// unconditionally, identical to the pre-role-aware behavior.
    ///
    /// **Warning:** if two modules register the same PID to different workflows
    /// and `all()` is active, the conflict guard in `PidRouter` will panic at
    /// build time. Use [`from_roles`] to specify exactly which roles apply.
    ///
    /// [`from_roles`]: DeploymentRoles::from_roles
    #[must_use]
    pub fn all() -> Self {
        Self {
            all: true,
            roles: HashSet::new(),
        }
    }

    /// Construct from an explicit set of active roles.
    ///
    /// Only modules whose role-conditional PID blocks include at least one of
    /// these roles will register those PIDs. All non-role-conditional PID blocks
    /// (i.e., those that don't call `roles.contains(...)`) are always registered.
    #[must_use]
    pub fn from_roles(roles: impl IntoIterator<Item = Marktrolle>) -> Self {
        Self {
            all: false,
            roles: roles.into_iter().collect(),
        }
    }

    /// Return `true` when `role` is active.
    ///
    /// Always returns `true` for [`DeploymentRoles::all()`].
    #[must_use]
    pub fn contains(&self, role: Marktrolle) -> bool {
        self.all || self.roles.contains(&role)
    }

    /// Return `true` when this is the [`all()`] sentinel (no explicit role list).
    ///
    /// [`all()`]: DeploymentRoles::all
    #[must_use]
    pub fn is_all(&self) -> bool {
        self.all
    }

    // ── Convenience constructors ──────────────────────────────────────────────

    /// NB-only deployment (most common for grid operators).
    #[must_use]
    pub fn nb() -> Self {
        Self::from_roles([Marktrolle::Nb])
    }

    /// ESA-only deployment (energy service provider side).
    #[must_use]
    pub fn esa() -> Self {
        Self::from_roles([Marktrolle::Esa])
    }

    /// LF-only deployment (supplier side).
    #[must_use]
    pub fn lf() -> Self {
        Self::from_roles([Marktrolle::Lf])
    }

    /// gMSB-only deployment (incumbent meter operator).
    #[must_use]
    pub fn msb() -> Self {
        Self::from_roles([Marktrolle::Msb])
    }

    /// nMSB-only deployment (challenger meter operator).
    #[must_use]
    pub fn nmsb() -> Self {
        Self::from_roles([Marktrolle::Nmsb])
    }

    /// NB + gMSB (most common municipal utility / Stadtwerke combination).
    #[must_use]
    pub fn nb_msb() -> Self {
        Self::from_roles([Marktrolle::Nb, Marktrolle::Msb])
    }

    /// NB + BKV (grid operator that also manages its own balance group).
    #[must_use]
    pub fn nb_bkv() -> Self {
        Self::from_roles([Marktrolle::Nb, Marktrolle::Bkv])
    }

    /// Add a role to an existing set, returning a new `DeploymentRoles`.
    #[must_use]
    pub fn with(mut self, role: Marktrolle) -> Self {
        if !self.all {
            self.roles.insert(role);
        }
        self
    }
}

impl FromIterator<Marktrolle> for DeploymentRoles {
    fn from_iter<T: IntoIterator<Item = Marktrolle>>(iter: T) -> Self {
        Self::from_roles(iter)
    }
}

#[cfg(test)]
mod esa_role_tests {
    use super::*;

    /// An ESA-only deployment activates exactly that role.
    #[test]
    fn esa_is_a_selectable_deployment_role() {
        let roles = DeploymentRoles::esa();
        assert!(roles.contains(Marktrolle::Esa));
        assert!(!roles.contains(Marktrolle::Msb));
        assert!(!roles.is_all());
    }

    /// An integrated deployment can be both: the MSB serves ESAs and the ESA
    /// arm consumes values. The two register disjoint PID sets.
    #[test]
    fn msb_and_esa_can_be_held_together() {
        let roles = DeploymentRoles::from_roles([Marktrolle::Msb, Marktrolle::Esa]);
        assert!(roles.contains(Marktrolle::Msb));
        assert!(roles.contains(Marktrolle::Esa));
    }
}
