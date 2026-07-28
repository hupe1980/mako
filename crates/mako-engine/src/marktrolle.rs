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
    /// Receives the GPKE Anmeldung ANFRAGE (55001 Lieferbeginn / 55002 Lieferende),
    /// issues ANTWORT messages (55003–55006), runs GPKE Konfiguration (17134/17135
    /// outbound ORDERS, 19001/19002 inbound ORDRSP). The Kündigung (55016/55017)
    /// is an LFN↔LFA exchange, not an NB ANFRAGE.
    Nb,

    /// Lieferant (LF) — energy supplier.
    ///
    /// Initiates GPKE Lieferbeginn/Lieferende, receives ANTWORT from NB.
    /// Registers as inbound-ANTWORT recipient (55003–55006/55018) for the
    /// LF-side anmeldung workflow.
    Lf,

    /// grundzuständiger Messstellenbetreiber (gMSB) — incumbent meter operator.
    ///
    /// In the WiM MSB-Wechsel (BK6-24-174) receives the Verpflichtungsanfrage/
    /// Aufforderung (55168, NB→gMSB); also handles WiM Zählerstand/Konfiguration
    /// (11001–11003, MSCONS/UTILTS). Often the same legal entity as the NB (§41 MsbG).
    Msb,

    /// nicht-grundzuständiger Messstellenbetreiber (nMSB) — challenger meter operator.
    ///
    /// Sends the WiM MSB-Wechsel Anmeldung (55042, MSBN→NB) and Kündigung MSB
    /// (55039, MSBN→MSBA), plus WiM Geräteübernahme ORDERS (17001, 17009).
    /// Receives inbound ORDRSP responses 19001/19002 (Bestellbestätigung/Ablehnung)
    /// and 19015/19016 (Gerätewechselabsicht).
    Nmsb,

    /// abgebender Messstellenbetreiber (aMSB) — outgoing meter operator.
    ///
    /// Receives the Kündigung MSB (55039, from the nMSB) and sends Ende MSB /
    /// Abmeldung (55051, MSBA→NB). This role is often held by the gMSB after a
    /// successful nMSB takeover.
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

    /// Gasnetzbetreiber (GNB) — gas network operator (GeLi Gas counterpart of NB).
    ///
    /// Receives GeLi Gas Lieferbeginn/Lieferende ANFRAGE messages (44001 ff.)
    /// and issues the corresponding ANTWORT messages (44003–44006).
    Gnb,

    /// Lieferant Gas (LFG) — gas supplier (GeLi Gas counterpart of LF).
    ///
    /// Initiates GeLi Gas Lieferbeginn/Lieferende (44001/44002) and receives
    /// the GNB's ANTWORT messages.
    Lfg,

    /// Lieferant neu (LFN) — the incoming supplier in a Lieferantenwechsel.
    ///
    /// Distinct from the generic [`Marktrolle::Lf`] where a process step is
    /// specific to the *gaining* side of a switch.
    Lfn,

    /// Lieferant alt (LFA) — the outgoing supplier in a Lieferantenwechsel.
    ///
    /// Distinct from the generic [`Marktrolle::Lf`] where a process step is
    /// specific to the *losing* side of a switch.
    Lfa,

    /// Marktgebietsverantwortlicher (MGV) — gas market-area manager.
    ///
    /// **Gas only.** Operates the Virtueller Handelspunkt and GaBi Gas
    /// balancing (THE in Germany). Declares its communication data via
    /// PARTIN 37011 ("Kommunikationsdaten des MGV Gas").
    Mgv,
}

impl Marktrolle {
    /// The canonical upper-case BDEW role code (e.g. `"NB"`, `"ÜNB"`, `"LFG"`).
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Nb => "NB",
            Self::Lf => "LF",
            Self::Msb => "MSB",
            Self::Nmsb => "NMSB",
            Self::Amsb => "AMSB",
            Self::Bkv => "BKV",
            Self::Uenb => "ÜNB",
            Self::Biko => "BIKO",
            Self::Esa => "ESA",
            Self::Gnb => "GNB",
            Self::Lfg => "LFG",
            Self::Lfn => "LFN",
            Self::Lfa => "LFA",
            Self::Mgv => "MGV",
        }
    }

    /// Parse a canonical upper-case BDEW role code back into a [`Marktrolle`].
    ///
    /// Round-trips [`as_code`] exactly (including the umlaut in `"ÜNB"`).
    /// Returns `None` for anything else — callers decide whether an unknown
    /// code is an error or simply "not one of ours".
    ///
    /// [`as_code`]: Marktrolle::as_code
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Some(match code {
            "NB" => Self::Nb,
            "LF" => Self::Lf,
            "MSB" => Self::Msb,
            "NMSB" => Self::Nmsb,
            "AMSB" => Self::Amsb,
            "BKV" => Self::Bkv,
            "ÜNB" => Self::Uenb,
            "BIKO" => Self::Biko,
            "ESA" => Self::Esa,
            "GNB" => Self::Gnb,
            "LFG" => Self::Lfg,
            "LFN" => Self::Lfn,
            "LFA" => Self::Lfa,
            "MGV" => Self::Mgv,
            _ => return None,
        })
    }

    /// Map a PARTIN Prüfidentifikator to the sender's [`Marktrolle`].
    ///
    /// PARTIN (PIDs 37000–37014) distributes market-participant communication
    /// data; the PID identifies the sender's role:
    ///
    /// | PID | Sender | `Marktrolle` |
    /// |---|---|---|
    /// | 37000 | LF Strom | [`Lf`](Self::Lf) |
    /// | 37001 | NB Strom | [`Nb`](Self::Nb) |
    /// | 37002 | MSB Strom | [`Msb`](Self::Msb) |
    /// | 37003 | BKV Strom | [`Bkv`](Self::Bkv) |
    /// | 37004 | BIKO Strom | [`Biko`](Self::Biko) |
    /// | 37005 | ÜNB Strom | [`Uenb`](Self::Uenb) |
    /// | 37006 | ESA Strom | [`Esa`](Self::Esa) |
    /// | 37008 | LF Gas | [`Lfg`](Self::Lfg) |
    /// | 37009 | NB Gas | [`Gnb`](Self::Gnb) |
    /// | 37010 | MSB Gas | [`Msb`](Self::Msb) |
    /// | 37011 | MGV Gas | [`Mgv`](Self::Mgv) |
    /// | 37012 | NB Gas (spartenübergreifend) | [`Gnb`](Self::Gnb) |
    /// | 37013 | MSB Gas (spartenübergreifend) | [`Msb`](Self::Msb) |
    /// | 37014 | MSB Strom (spartenübergreifend) | [`Msb`](Self::Msb) |
    ///
    /// Returns `None` for unrecognised codes (37007 is a gap in the AHB).
    #[must_use]
    pub fn from_partin_pid(pid: u32) -> Option<Self> {
        match pid {
            37000 => Some(Self::Lf),
            37001 => Some(Self::Nb),
            37002 | 37010 | 37013 | 37014 => Some(Self::Msb),
            37003 => Some(Self::Bkv),
            37004 => Some(Self::Biko),
            37005 => Some(Self::Uenb),
            37006 => Some(Self::Esa),
            37008 => Some(Self::Lfg),
            37009 | 37012 => Some(Self::Gnb),
            37011 => Some(Self::Mgv),
            _ => None,
        }
    }
}

// Serde representation: the canonical BDEW role code (`"NB"`, `"ÜNB"`, `"LFG"`, …).
// Used verbatim in persisted partner records and API payloads, so the wire
// format matches EDIFACT/BO4E role codes exactly.
impl serde::Serialize for Marktrolle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_code())
    }
}

impl<'de> serde::Deserialize<'de> for Marktrolle {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = String::deserialize(deserializer)?;
        Self::from_code(&code)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown Marktrolle code {code:?}")))
    }
}

impl std::fmt::Display for Marktrolle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_code())
    }
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

// ── Command licensing ─────────────────────────────────────────────────────────

/// Why [`resolve_role`] rejected a command submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicensingError {
    /// The command permits several roles and the caller asserted none —
    /// the engine cannot infer which hat the caller is wearing.
    MarktrolleRequired,
    /// The asserted role is not in the command's permitted set.
    RoleNotPermitted,
    /// The effective role is not among the deployment's configured roles.
    RoleNotConfigured,
}

impl std::fmt::Display for LicensingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MarktrolleRequired => {
                f.write_str("multi-role command requires an asserted Marktrolle")
            }
            Self::RoleNotPermitted => {
                f.write_str("asserted Marktrolle is not permitted for this command")
            }
            Self::RoleNotConfigured => {
                f.write_str("deployment is not configured for the required Marktrolle")
            }
        }
    }
}

impl std::error::Error for LicensingError {}

/// Resolve and validate the effective [`Marktrolle`] for a command submission.
///
/// Pure licensing policy — no registry lookup, no I/O:
///
/// - **Single-role commands** (`permitted.len() == 1`): the role is inferred
///   from the permitted set; any `asserted` role is deliberately **ignored**
///   so ERP connectors that always send a fixed role are not rejected.
/// - **Multi-role commands** (`permitted.len() != 1`): `asserted` must be
///   `Some` ([`LicensingError::MarktrolleRequired`]) and must be a member of
///   `permitted` ([`LicensingError::RoleNotPermitted`]).
///
/// The effective role is then cross-checked against the deployment
/// configuration: [`DeploymentRoles::all`] admits every role; an explicit
/// (possibly empty) role set admits only its members
/// ([`LicensingError::RoleNotConfigured`]).
///
/// # Errors
///
/// See [`LicensingError`] for the three rejection reasons.
pub fn resolve_role(
    permitted: &[Marktrolle],
    asserted: Option<Marktrolle>,
    configured: &DeploymentRoles,
) -> Result<Marktrolle, LicensingError> {
    let effective = if permitted.len() == 1 {
        // Single-role command — fully implied; asserted role is ignored.
        permitted[0]
    } else {
        let r = asserted.ok_or(LicensingError::MarktrolleRequired)?;
        if !permitted.contains(&r) {
            return Err(LicensingError::RoleNotPermitted);
        }
        r
    };

    if !configured.contains(effective) {
        return Err(LicensingError::RoleNotConfigured);
    }

    Ok(effective)
}

#[cfg(test)]
mod licensing_tests {
    use super::*;

    #[test]
    fn code_round_trip_for_every_role() {
        for role in [
            Marktrolle::Nb,
            Marktrolle::Lf,
            Marktrolle::Msb,
            Marktrolle::Nmsb,
            Marktrolle::Amsb,
            Marktrolle::Bkv,
            Marktrolle::Uenb,
            Marktrolle::Biko,
            Marktrolle::Esa,
            Marktrolle::Gnb,
            Marktrolle::Lfg,
            Marktrolle::Lfn,
            Marktrolle::Lfa,
            Marktrolle::Mgv,
        ] {
            assert_eq!(Marktrolle::from_code(role.as_code()), Some(role));
        }
        assert_eq!(Marktrolle::from_code("ÜNB"), Some(Marktrolle::Uenb));
        assert_eq!(
            Marktrolle::from_code("nb"),
            None,
            "codes are case-sensitive"
        );
        assert_eq!(Marktrolle::from_code(""), None);
    }

    #[test]
    fn serde_round_trips_as_bdew_code() {
        for role in [Marktrolle::Nb, Marktrolle::Uenb, Marktrolle::Lfg] {
            let json = serde_json::to_string(&role).unwrap();
            assert_eq!(json, format!("\"{}\"", role.as_code()));
            let back: Marktrolle = serde_json::from_str(&json).unwrap();
            assert_eq!(back, role);
        }
        assert!(serde_json::from_str::<Marktrolle>("\"LfStrom\"").is_err());
    }

    #[test]
    fn from_partin_pid_covers_all_partin_pids() {
        for pid in [
            37000u32, 37001, 37002, 37003, 37004, 37005, 37006, 37008, 37009, 37010, 37011, 37012,
            37013, 37014,
        ] {
            assert!(
                Marktrolle::from_partin_pid(pid).is_some(),
                "from_partin_pid({pid}) should return Some"
            );
        }
        assert_eq!(Marktrolle::from_partin_pid(37000), Some(Marktrolle::Lf));
        assert_eq!(Marktrolle::from_partin_pid(37008), Some(Marktrolle::Lfg));
        assert_eq!(Marktrolle::from_partin_pid(37009), Some(Marktrolle::Gnb));
        assert_eq!(Marktrolle::from_partin_pid(37011), Some(Marktrolle::Mgv));
        assert_eq!(Marktrolle::from_partin_pid(37014), Some(Marktrolle::Msb));
        // PID 37007 is not in the AHB (gap)
        assert_eq!(Marktrolle::from_partin_pid(37007), None);
        assert_eq!(Marktrolle::from_partin_pid(0), None);
    }

    #[test]
    fn single_permitted_infers_and_ignores_assertion() {
        let configured = DeploymentRoles::lf();
        // No assertion → inferred.
        assert_eq!(
            resolve_role(&[Marktrolle::Lf], None, &configured),
            Ok(Marktrolle::Lf)
        );
        // A wrong assertion is ignored, not rejected.
        assert_eq!(
            resolve_role(&[Marktrolle::Lf], Some(Marktrolle::Nb), &configured),
            Ok(Marktrolle::Lf)
        );
    }

    #[test]
    fn multi_permitted_requires_assertion() {
        let permitted = [Marktrolle::Nb, Marktrolle::Msb];
        let configured = DeploymentRoles::nb_msb();
        assert_eq!(
            resolve_role(&permitted, None, &configured),
            Err(LicensingError::MarktrolleRequired)
        );
        assert_eq!(
            resolve_role(&permitted, Some(Marktrolle::Msb), &configured),
            Ok(Marktrolle::Msb)
        );
    }

    #[test]
    fn multi_permitted_rejects_foreign_assertion() {
        let permitted = [Marktrolle::Nb, Marktrolle::Msb];
        let configured = DeploymentRoles::lf();
        assert_eq!(
            resolve_role(&permitted, Some(Marktrolle::Lf), &configured),
            Err(LicensingError::RoleNotPermitted)
        );
    }

    #[test]
    fn configured_cross_check_rejects_unconfigured_role() {
        // Resolves to LF; only NB is configured.
        assert_eq!(
            resolve_role(&[Marktrolle::Lf], None, &DeploymentRoles::nb()),
            Err(LicensingError::RoleNotConfigured)
        );
        // Empty explicit set admits nothing.
        assert_eq!(
            resolve_role(&[Marktrolle::Lf], None, &DeploymentRoles::from_roles([])),
            Err(LicensingError::RoleNotConfigured)
        );
    }

    #[test]
    fn deployment_roles_all_admits_every_role() {
        assert_eq!(
            resolve_role(&[Marktrolle::Biko], None, &DeploymentRoles::all()),
            Ok(Marktrolle::Biko)
        );
        assert_eq!(
            resolve_role(
                &[Marktrolle::Bkv, Marktrolle::Uenb],
                Some(Marktrolle::Uenb),
                &DeploymentRoles::all()
            ),
            Ok(Marktrolle::Uenb)
        );
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
