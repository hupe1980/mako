//! GLN registry — maps BDEW Marktrollen to their operator GLNs for this instance.
//!
//! ## BDEW §2.13 — Marktpartneridentifikation
//!
//! Per the BDEW EDI@Energy General Provisions (§2.13, V6.1d 01.04.2026):
//!
//! > "Marktteilnehmer benötigen für jede Marktrolle eine gesonderte Codenummer."
//! > "Identifiziert sich ein Marktteilnehmer über GLN und ist er in beiden
//! > Branchen tätig, so muss er je Energieart und Marktrolle verschiedene
//! > GLN nutzen."
//!
//! Operationally this means:
//!
//! - Every `[[party]]` entry must cover exactly **one Sparte** (Strom *or* Gas).
//!   Mixing Strom roles (`NB`, `LF`, `MSB`, …) with Gas roles (`GNB`, `LFG`,
//!   `GMSB`, …) in a single `[[party]]` entry is rejected at startup.
//! - BDEW issues **separate codes per Marktrolle**: a company acting as both
//!   `NB` and `MSB` has two different BDEW-Codenummern and therefore two
//!   separate `[[party]]` entries with different GLNs.
//! - Sparte-neutral roles (`RB` — Registerbetreiber) may coexist with either
//!   Strom or Gas roles in a single entry without triggering the §2.13 check.
//!
//! ## MP-ID formats and NAD agency codes
//!
//! | ID type | Prefix | Digits | NAD DE3055 | UNB DE0007 | Registry |
//! |---|---|---|---|---|---|
//! | BDEW-Codenummer (Strom) | `99` | 13 | `293` | `500` | bdew-codes.de |
//! | DVGW-Codenummer (Gas)   | `98` | 13 | `332` | `502` | codevergabe.dvgw-sc.de |
//! | GLN (GS1)               | varies | 13 | `9` | `14` | GS1 |
//! | EIC                     | — | 16 | `ZEW` | — | ENTSO-E |
//!
//! Source: BDEW-AWH Identifikatoren V1.2 §2.2; Allgemeine Festlegungen V6.1d
//! §2.13; UTILMD AHB Gas 1.2 NAD+MS/MR tables.
//!
//! The `agency` field in `[[party]]` overrides the auto-derived code.
//! When omitted, the NAD DE3055 code is derived from the GLN prefix (see above).
//!
//! ## Configuration
//!
//! Every `makod` deployment requires at least one `[[party]]` entry:
//!
//! ```toml
//! # Strom NB:
//! [[party]]
//! gln     = "9900001000001"   # BDEW-Codenummer (99…) → agency "293" auto-derived
//! roles   = ["NB"]
//! primary = true              # storage partition key + default sender
//!
//! # Strom LF — separate BDEW code per role (§2.13):
//! [[party]]
//! gln   = "9900001000002"
//! roles = ["LF"]
//!
//! # Strom gMSB — separate BDEW code (BDEW issues one code per Marktrolle):
//! [[party]]
//! gln   = "9900001000003"
//! roles = ["MSB"]
//!
//! # Gas GNB — MUST have a different GLN from all Strom entries (§2.13):
//! [[party]]
//! gln   = "9800001000001"     # DVGW-Codenummer (98…) → agency "332" auto-derived
//! roles = ["GNB"]
//!
//! # Gas LFG — separate DVGW code per role:
//! [[party]]
//! gln   = "9800001000002"
//! roles = ["LFG"]
//!
//! # GS1 GLN — agency auto-derived to "9"; override only if prefix is ambiguous:
//! [[party]]
//! gln   = "4012345000023"     # GS1 GLN (non-98/99 prefix) → agency "9"
//! roles = ["RB"]              # Registerbetreiber — sparte-neutral
//! ```
//!
//! ## Roles without engine deployment routing
//!
//! The following roles are valid in `[[party]]` but have no PID routing in the
//! current engine version.  They are accepted at startup and stored in the
//! registry but never appear in [`MpIdRegistry::deployment_role_strings`]:
//!
//! | Role | Reason |
//! |---|---|
//! | `MGV` | Marktgebietsverantwortlicher — GaBi Gas PIDs are registered unconditionally |
//! | `DP`  | Data Provider Strom — UTILTS metering distribution, placeholder |
//! | `EIV` | Einsatzverantwortlicher Strom — Redispatch 2.0, placeholder crate |
//! | `ESA` | Energieserviceanbieter — iMS / smart meter, placeholder |
//! | `KN`  | Kapazitätsnutzer Gas — GaBi Gas capacity booking, placeholder |
//! | `RB`  | Registerbetreiber — MaStR data registry, placeholder |
//!
//! ## Key properties
//!
//! - **AS4 loopback detection** — [`is_own_gln`] returns `true` for any GLN
//!   that belongs to this operator, enabling in-process delivery for combined-role
//!   workflows (NB→MSB, GNB→gMSB) regardless of which GLN each role uses.
//!
//! - **EDIFACT sender selection** — [`sender_gln_for_orders_pid`] returns the
//!   correct sender GLN for ORDERS messages using a static PID → role table.
//!
//! - **Deployment role derivation** — [`deployment_role_strings`] normalises the
//!   `[[party]]` roles into the strings accepted by `parse_deployment_roles`,
//!   enabling auto-derivation of `--deployment-roles` and `--marktrollen`.
//!
//! [`is_own_gln`]: MpIdRegistry::is_own_gln
//! [`sender_gln_for_orders_pid`]: MpIdRegistry::sender_gln_for_orders_pid
//! [`deployment_role_strings`]: MpIdRegistry::deployment_role_strings

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::config::PartyConfig;

// ── Role table ────────────────────────────────────────────────────────────────

/// Sparte (energy sector) classification of a BDEW Marktrolle.
///
/// Source: BDEW Rollenmodell V2.2 (08.01.2026).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleSparte {
    /// Role belongs to the electricity sector only.
    Strom,
    /// Role belongs to the gas sector only.
    Gas,
    /// Role is valid in both sectors (e.g. `RB` — Registerbetreiber).
    Both,
}

/// Metadata for a single BDEW Marktrolle entry in `ROLE_TABLE`.
struct RoleEntry {
    /// Normalised uppercase abbreviation (used in `[[party]]` config).
    abbrev: &'static str,
    /// Energy sector.
    sparte: RoleSparte,
    /// Canonical string for `parse_deployment_roles` / `--deployment-roles`.
    ///
    /// `None` = role has no active PID routing in the current engine version;
    /// accepted at startup but excluded from `deployment_role_strings()`.
    ///
    /// Some roles canonicalise to a different string (e.g. `GNB` → `"NB"`)
    /// because the engine uses one `Marktrolle` for both Strom and Gas sectors.
    engine_canonical: Option<&'static str>,
}

/// Authoritative BDEW role table — single source of truth.
///
/// Replaces the previous trio of `KNOWN_ROLES`, `STROM_ROLES`, `GAS_ROLES`
/// arrays that required manual three-way sync on every role addition.  All
/// validation, §2.13 sparte checks, and `deployment_role_strings` now derive
/// from this table automatically.
///
/// Source: BDEW Rollenmodell V2.2 (08.01.2026) — only roles with
/// `Marktkommunikation: zur Verwendung freigegeben`, plus EDIFACT AHB
/// sub-qualifiers (`GNB`, `LFG`, `GMSB`, `ANB`, `VNB`, `NMSB`, `AMSB`, `FNB`)
/// which appear in NAD fields and are accepted in `[[party]]` config.
static ROLE_TABLE: &[RoleEntry] = &[
    // ── Strom ──────────────────────────────────────────────────────────────
    RoleEntry {
        abbrev: "NB",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("NB"),
    },
    RoleEntry {
        abbrev: "LF",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("LF"),
    },
    RoleEntry {
        abbrev: "MSB",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("MSB"),
    },
    RoleEntry {
        abbrev: "ANB",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("NB"),
    }, // Anschlussnehmer-NB → Nb
    RoleEntry {
        abbrev: "VNB",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("NB"),
    }, // Verteilnetzbetreiber → Nb
    RoleEntry {
        abbrev: "NMSB",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("NMSB"),
    },
    RoleEntry {
        abbrev: "AMSB",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("AMSB"),
    },
    RoleEntry {
        abbrev: "BKV",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("BKV"),
    },
    RoleEntry {
        abbrev: "UNB",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("UNB"),
    },
    RoleEntry {
        abbrev: "BIKO",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("BIKO"),
    },
    RoleEntry {
        abbrev: "DP",
        sparte: RoleSparte::Strom,
        engine_canonical: None,
    }, // Data Provider — no PID routing yet
    RoleEntry {
        abbrev: "EIV",
        sparte: RoleSparte::Strom,
        engine_canonical: None,
    }, // Einsatzverantwortlicher — Redispatch 2.0 placeholder
    RoleEntry {
        abbrev: "ESA",
        sparte: RoleSparte::Strom,
        engine_canonical: Some("ESA"),
    }, // Energieserviceanbieter (PARTIN 37006) — WiM Teil 2 Kap. 4
    // ── Gas ────────────────────────────────────────────────────────────────
    RoleEntry {
        abbrev: "GNB",
        sparte: RoleSparte::Gas,
        engine_canonical: Some("NB"),
    }, // Gasnetzbetreiber → Nb
    RoleEntry {
        abbrev: "LFG",
        sparte: RoleSparte::Gas,
        engine_canonical: Some("LF"),
    }, // Lieferant Gas → Lf
    RoleEntry {
        abbrev: "GMSB",
        sparte: RoleSparte::Gas,
        engine_canonical: Some("MSB"),
    }, // grundzust. MSB Gas → Msb
    RoleEntry {
        abbrev: "MGV",
        sparte: RoleSparte::Gas,
        engine_canonical: None,
    }, // Marktgebietsverantwortlicher — no routing
    RoleEntry {
        abbrev: "FNB",
        sparte: RoleSparte::Gas,
        engine_canonical: Some("UNB"),
    }, // Fernleitungsnetzbetreiber (Gas TSO) → Uenb
    RoleEntry {
        abbrev: "KN",
        sparte: RoleSparte::Gas,
        engine_canonical: None,
    }, // Kapazitätsnutzer — GaBi Gas placeholder
    // ── Both spartes ────────────────────────────────────────────────────────
    RoleEntry {
        abbrev: "RB",
        sparte: RoleSparte::Both,
        engine_canonical: None,
    }, // Registerbetreiber — MaStR placeholder
];

/// Look up role metadata for the given abbreviation (must already be uppercased).
fn find_role(upper: &str) -> Option<&'static RoleEntry> {
    ROLE_TABLE.iter().find(|e| e.abbrev == upper)
}

/// Returns the [`RoleSparte`] for the given role abbreviation (case-insensitive).
///
/// Returns `None` when the abbreviation is not a known BDEW Marktrolle.
/// Useful for external callers (e.g. `marktd` event routing) that need to
/// determine a role's energy sector without constructing a full registry.
#[must_use]
#[allow(dead_code)] // public API for marktd; not yet called within the binary
pub fn sparte_for_role(abbrev: &str) -> Option<RoleSparte> {
    find_role(&abbrev.to_uppercase()).map(|e| e.sparte)
}

// ── NAD agency derivation ─────────────────────────────────────────────────────

/// Fallback NAD DE3055 agency code used only when a GLN is not in the registry.
///
/// In practice every GLN goes through `derive_agency` in `from_config`.
const DEFAULT_AGENCY: &str = "293"; // BDEW-Codenummer Strom

/// Derive the NAD DE3055 agency code from the MP-ID prefix.
///
/// Implements BDEW-AWH Identifikatoren V1.2 §2.2:
///
/// | Length | Prefix | NAD DE3055 | Meaning |
/// |--------|--------|------------|---------|
/// | 13 | `99` | `"293"` | BDEW-Codenummer Strom |
/// | 13 | `98` | `"332"` | DVGW-Codenummer Gas |
/// | 13 | other | `"9"` | GS1 GLN |
/// | 16 | — | `"ZEW"` | EIC (ENTSO-E) |
///
/// Note: UNB DE0007 codes differ — `500` (BDEW), `502` (DVGW), `14` (GS1).
fn derive_agency(mp_id: &str) -> &'static str {
    match mp_id.len() {
        13 if mp_id.starts_with("99") => "293",
        13 if mp_id.starts_with("98") => "332",
        13 => "9",
        _ => "ZEW",
    }
}

// ── MpIdRegistry ───────────────────────────────────────────────────────────────

/// Role → GLN mapping for this `makod` instance.
///
/// Built at startup from `[[party]]` entries in `makod.toml` via
/// [`MpIdRegistry::from_config`].  At least one entry is required.
#[derive(Debug, Clone)]
pub struct MpIdRegistry {
    /// Primary GLN (storage partition key / default sender).
    primary_gln: Arc<str>,
    /// NAD DE3055 agency code for the primary GLN.
    primary_agency: Arc<str>,
    /// All own GLNs — for loopback detection.
    own_glns: HashSet<Arc<str>>,
    /// Normalised role (uppercase) → GLN.
    role_to_gln: HashMap<Box<str>, Arc<str>>,
    /// GLN → NAD DE3055 agency code.
    #[allow(dead_code)]
    gln_to_agency: HashMap<Arc<str>, Arc<str>>,
    /// All declared roles normalised to uppercase, deduplicated, sorted.
    ///
    /// Used by [`deployment_role_strings`] for auto-deriving engine roles.
    ///
    /// [`deployment_role_strings`]: Self::deployment_role_strings
    all_roles: Vec<Box<str>>,
}

impl MpIdRegistry {
    // ── Constructor ───────────────────────────────────────────────────────────

    /// Build from `[[party]]` config entries.
    ///
    /// # Errors
    ///
    /// Returns an error when:
    ///
    /// - `parties` is empty.
    /// - A GLN is not a valid 13-digit BDEW/DVGW/GS1 code or 16-char EIC.
    /// - Two entries share the same GLN (each Marktrolle requires its own code).
    /// - A role appears in more than one entry.
    /// - More than one entry has `primary = true`.
    /// - A role string is not a known BDEW Marktrolle.
    /// - A single entry mixes Strom roles with Gas roles (BDEW §2.13).
    ///
    /// # Primary selection
    ///
    /// The first entry with `primary = true` is used as the storage partition
    /// key and default sender GLN.  When no entry carries `primary = true`,
    /// the first entry in document order is used.
    pub fn from_config(parties: &[PartyConfig]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !parties.is_empty(),
            "makod.toml requires at least one [[party]] entry.\n\
             \n\
             [[party]]\n\
             mp_id     = \"<13-digit BDEW/DVGW code or GS1 GLN>\"\n\
             roles   = [\"NB\"]  # or LF, MSB, GNB, LFG, …\n\
             primary = true"
        );

        let primary_count = parties.iter().filter(|p| p.primary).count();
        anyhow::ensure!(
            primary_count <= 1,
            "at most one [[party]] entry may have `primary = true` (found {})",
            primary_count
        );

        let mut seen_glns: HashSet<&str> = HashSet::new();
        let mut seen_roles: HashMap<Box<str>, &str> = HashMap::new();

        for party in parties {
            validate_gln(&party.mp_id)?;

            if !seen_glns.insert(party.mp_id.as_str()) {
                anyhow::bail!(
                    "duplicate GLN {:?} — each [[party]] entry must have a unique GLN \
                     (BDEW issues separate Codenummern per Marktrolle; use separate \
                     entries with different GLNs)",
                    party.mp_id
                );
            }

            // Validate each role and accumulate sparte info for the §2.13 check.
            // validate_role returns the &RoleEntry, avoiding a second table scan.
            let mut strom_roles: Vec<&str> = Vec::new();
            let mut gas_roles: Vec<&str> = Vec::new();

            for role in &party.roles {
                let upper = role.to_uppercase();
                let entry = validate_role(&upper)
                    .map_err(|e| anyhow::anyhow!("in [[party]] mp_id = {:?}: {e}", party.mp_id))?;

                let key: Box<str> = upper.into_boxed_str();
                if let Some(prev_gln) = seen_roles.get(&key) {
                    anyhow::bail!(
                        "role {:?} is claimed by {:?} and {:?} — \
                         each Marktrolle must belong to exactly one [[party]] entry",
                        role,
                        prev_gln,
                        party.mp_id
                    );
                }
                seen_roles.insert(key, party.mp_id.as_str());

                match entry.sparte {
                    RoleSparte::Strom => strom_roles.push(role.as_str()),
                    RoleSparte::Gas => gas_roles.push(role.as_str()),
                    RoleSparte::Both => {} // sparte-neutral; never triggers the mix check
                }
            }

            // §2.13: a single [[party]] entry must not mix Strom and Gas roles.
            if !strom_roles.is_empty() && !gas_roles.is_empty() {
                anyhow::bail!(
                    "[[party]] mp_id = {:?} mixes Strom roles {strom_roles:?} with Gas \
                     roles {gas_roles:?}.\n\
                     Per BDEW §2.13 (Allgemeine Festlegungen V6.1d), each Marktrolle \
                     requires a separate MP-ID; operators active in both sectors must \
                     use different GLNs per energy type and role. Use separate \
                     [[party]] entries — one for Strom (BDEW code, 99…) and one for \
                     Gas (DVGW code, 98…).",
                    party.mp_id,
                );
            }
        }

        // ── Build the registry ────────────────────────────────────────────────
        let primary = parties
            .iter()
            .find(|p| p.primary)
            .or_else(|| parties.first())
            .expect("non-empty — checked above");

        let primary_gln: Arc<str> = primary.mp_id.as_str().into();
        let primary_agency: Arc<str> = primary
            .agency
            .as_deref()
            .unwrap_or_else(|| derive_agency(&primary.mp_id))
            .into();

        let mut own_glns: HashSet<Arc<str>> = HashSet::new();
        let mut role_to_gln: HashMap<Box<str>, Arc<str>> = HashMap::new();
        let mut gln_to_agency: HashMap<Arc<str>, Arc<str>> = HashMap::new();
        let mut all_roles: Vec<Box<str>> = Vec::new();

        for party in parties {
            let gln_arc: Arc<str> = party.mp_id.as_str().into();
            let agency: Arc<str> = party
                .agency
                .as_deref()
                .unwrap_or_else(|| derive_agency(&party.mp_id))
                .into();
            own_glns.insert(Arc::clone(&gln_arc));
            gln_to_agency.insert(Arc::clone(&gln_arc), agency);
            for role in &party.roles {
                let key: Box<str> = role.to_uppercase().into_boxed_str();
                all_roles.push(key.clone());
                role_to_gln.insert(key, Arc::clone(&gln_arc));
            }
        }
        all_roles.sort_unstable();
        all_roles.dedup();

        Ok(Self {
            primary_gln,
            primary_agency,
            own_glns,
            role_to_gln,
            gln_to_agency,
            all_roles,
        })
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Returns the primary GLN (storage partition key / default sender).
    #[must_use]
    pub fn primary_gln(&self) -> &str {
        &self.primary_gln
    }

    /// Returns the NAD DE3055 agency code for the primary GLN.
    ///
    /// Auto-derived from the GLN prefix when not set explicitly in config:
    /// `99…` → `"293"` (BDEW), `98…` → `"332"` (DVGW), other 13-digit → `"9"` (GS1),
    /// 16-char → `"ZEW"` (EIC).
    #[must_use]
    pub fn primary_agency(&self) -> &str {
        &self.primary_agency
    }

    /// Returns the GLN for the given BDEW Marktrolle (case-insensitive).
    ///
    /// Returns `None` when no `[[party]]` entry declares this role.
    #[must_use]
    pub fn gln_for_role(&self, role: &str) -> Option<&str> {
        self.role_to_gln
            .get(role.to_uppercase().as_str())
            .map(Arc::as_ref)
    }

    /// Returns the GLN for the given BDEW Marktrolle, or [`primary_gln`] as fallback.
    ///
    /// [`primary_gln`]: MpIdRegistry::primary_gln
    #[must_use]
    pub fn gln_for_role_or_primary(&self, role: &str) -> &str {
        self.gln_for_role(role).unwrap_or(self.primary_gln())
    }

    /// Returns the NAD DE3055 agency code for the given GLN.
    ///
    /// Uses the configured or auto-derived value for known GLNs;
    /// falls back to `"293"` (BDEW Strom) for unknown GLNs.
    #[must_use]
    #[allow(dead_code)]
    pub fn agency_for_gln(&self, mp_id: &str) -> &str {
        self.gln_to_agency
            .get(mp_id)
            .map(Arc::as_ref)
            .unwrap_or(DEFAULT_AGENCY)
    }

    /// Returns `true` when the given GLN belongs to this operator.
    ///
    /// Used by the AS4 loopback path: a message addressed to an own GLN is
    /// delivered in-process rather than over the network.  Covers ALL own GLNs
    /// so loopback works even when `NB` and `MSB` have different GLNs on the
    /// same `makod` instance.
    #[must_use]
    pub fn is_own_gln(&self, mp_id: &str) -> bool {
        self.own_glns.contains(mp_id)
    }

    /// Iterates over all own GLNs (one per `[[party]]` entry).
    pub fn own_glns(&self) -> impl Iterator<Item = &str> {
        self.own_glns.iter().map(Arc::as_ref)
    }

    /// All declared BDEW Marktrollen, normalised to uppercase, sorted.
    ///
    /// Used to auto-derive `--deployment-roles` / `--marktrollen` when those
    /// flags are not set explicitly on the CLI.
    #[must_use]
    pub fn all_roles(&self) -> &[Box<str>] {
        &self.all_roles
    }

    /// BDEW Marktrollen normalised to the canonical strings accepted by
    /// `parse_deployment_roles` and the `--marktrollen` / `--deployment-roles`
    /// CLI flags.
    ///
    /// Derived from `ROLE_TABLE`.  Gas sub-qualifiers map to their
    /// Strom-canonical engine role name where the engine uses one `Marktrolle`
    /// for both sectors:
    ///
    /// | Config role | Canonical | Engine `Marktrolle` |
    /// |---|---|---|
    /// | `GNB`, `ANB`, `VNB` | `NB` | `Nb` |
    /// | `LFG` | `LF` | `Lf` |
    /// | `GMSB` | `MSB` | `Msb` |
    /// | `FNB` | `UNB` | `Uenb` (Gas TSO) |
    ///
    /// Roles with `engine_canonical = None` (`MGV`, `DP`, `EIV`, `ESA`, `KN`,
    /// `RB`) are excluded — they have no active PID routing.
    #[must_use]
    pub fn deployment_role_strings(&self) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        for role in &self.all_roles {
            let Some(entry) = find_role(role) else {
                continue; // cannot happen after from_config validation
            };
            let Some(canonical) = entry.engine_canonical else {
                continue; // role has no engine deployment role
            };
            let s = canonical.to_owned();
            if !result.contains(&s) {
                result.push(s);
            }
        }
        result
    }

    // ── ORDERS sender resolution ──────────────────────────────────────────────

    /// Best-effort sender GLN for ORDERS messages that do not embed `"sender"`.
    ///
    /// Uses a static PID → sending-role table derived from the BDEW AHB PID
    /// overview.  Falls back to [`primary_gln`] when the role is not configured
    /// or the PID is unknown.
    ///
    /// **Ambiguous PIDs** (shared by both Strom and Gas roles with potentially
    /// different GLNs) emit a `warn!` log and fall back to [`primary_gln`].
    /// Set `"sender"` explicitly in the ORDERS payload to resolve the ambiguity.
    ///
    /// [`primary_gln`]: MpIdRegistry::primary_gln
    #[must_use]
    pub fn sender_gln_for_orders_pid(&self, pid: u32) -> &str {
        match pid {
            // ── Sperrung / Entsperrung (PIDs 17115–17117) ──────────────────
            // LF initiates Sperrung Strom; LFG initiates Sperrung Gas.
            17115 | 17117 => self.resolve_ambiguous(pid, "LF", "LFG"),
            // NB / GNB issues Entsperrung / MSB-Beauftragung.
            17116 => self.resolve_ambiguous(pid, "NB", "GNB"),

            // ── GPKE Konfigurationseinrichtung (NB → MSB, Teil 3) ───────────
            17134 | 17135 => self.gln_for_role_or_primary("NB"),

            // ── WiM Geräteübernahme (NB → MSB / MSBA) ──────────────────────
            17001..=17011 => self.gln_for_role_or_primary("NB"),

            // ── Datenabruf / Reklamation (LF → NB/MSB) ─────────────────────
            17102 | 17113 => self.gln_for_role_or_primary("LF"),

            // ── Allokationsliste Gas (LF → NB) ──────────────────────────────
            17110 | 17114 => self.gln_for_role_or_primary("LF"),

            // ── GPKE Konfigurationsänderung (LF → NB/MSB, Teil 3) ──────────
            17120..=17133 => self.gln_for_role_or_primary("LF"),

            // ── Gas Datenabruf (LFG or GNB) ─────────────────────────────────
            17103 | 17104 => self.resolve_ambiguous(pid, "LFG", "GNB"),

            _ => self.primary_gln(),
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn resolve_ambiguous(&self, pid: u32, role_a: &str, role_b: &str) -> &str {
        match (self.gln_for_role(role_a), self.gln_for_role(role_b)) {
            (Some(a), Some(b)) if a == b => a,
            (Some(_), Some(_)) => {
                tracing::warn!(
                    pid,
                    role_a,
                    role_b,
                    "ORDERS sender GLN is ambiguous: {role_a} and {role_b} have \
                     different GLNs. Set \"sender\" in the ORDERS payload to resolve. \
                     Falling back to primary_gln.",
                );
                self.primary_gln()
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => self.primary_gln(),
        }
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

/// Validate a BDEW/DVGW MP-ID (13 ASCII digits) or EIC (16 alphanumeric chars).
fn validate_gln(mp_id: &str) -> anyhow::Result<()> {
    match mp_id.len() {
        13 if mp_id.bytes().all(|b| b.is_ascii_digit()) => Ok(()),
        16 if mp_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
        {
            Ok(())
        }
        _ => anyhow::bail!(
            "GLN {:?} is not a valid 13-digit BDEW/DVGW/GS1 code or 16-char EIC.\n\
             Examples: BDEW \"9900001000001\", DVGW \"9800001000001\", GS1 \"4012345000023\"",
            mp_id
        ),
    }
}

/// Validate that `upper_role` is a known BDEW Marktrolle (already uppercased).
///
/// Returns the static [`RoleEntry`] on success, avoiding a second table lookup
/// in the caller (sparte, engine_canonical immediately available).
fn validate_role(upper_role: &str) -> anyhow::Result<&'static RoleEntry> {
    find_role(upper_role).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown BDEW Marktrolle {:?}.\n\
             Strom: NB, LF, MSB, ANB, VNB, NMSB, AMSB, BKV, UNB, BIKO, DP, EIV, ESA\n\
             Gas:   GNB, LFG, GMSB, MGV, FNB, KN\n\
             Both:  RB",
            upper_role,
        )
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PartyConfig;

    fn party(mp_id: &str, roles: &[&str], primary: bool) -> PartyConfig {
        PartyConfig {
            mp_id: mp_id.to_owned(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
            primary,
            agency: None,
        }
    }

    // ── ROLE_TABLE invariants ─────────────────────────────────────────────────

    #[test]
    fn role_table_no_duplicate_abbrevs() {
        let mut seen = std::collections::HashSet::new();
        for e in ROLE_TABLE {
            assert!(
                seen.insert(e.abbrev),
                "duplicate abbrev in ROLE_TABLE: {}",
                e.abbrev
            );
        }
    }

    #[test]
    fn sparte_for_role_is_correct() {
        assert_eq!(sparte_for_role("NB"), Some(RoleSparte::Strom));
        assert_eq!(sparte_for_role("nb"), Some(RoleSparte::Strom)); // case-insensitive
        assert_eq!(sparte_for_role("GNB"), Some(RoleSparte::Gas));
        assert_eq!(sparte_for_role("FNB"), Some(RoleSparte::Gas));
        assert_eq!(sparte_for_role("RB"), Some(RoleSparte::Both));
        assert_eq!(sparte_for_role("UNKNOWN"), None);
    }

    #[test]
    fn all_strom_roles_have_strom_sparte() {
        for r in [
            "NB", "LF", "MSB", "ANB", "VNB", "NMSB", "AMSB", "BKV", "UNB", "BIKO", "DP", "EIV",
            "ESA",
        ] {
            assert_eq!(
                sparte_for_role(r),
                Some(RoleSparte::Strom),
                "{r} should be Strom"
            );
        }
    }

    #[test]
    fn all_gas_roles_have_gas_sparte() {
        for r in ["GNB", "LFG", "GMSB", "MGV", "FNB", "KN"] {
            assert_eq!(
                sparte_for_role(r),
                Some(RoleSparte::Gas),
                "{r} should be Gas"
            );
        }
    }

    // ── from_config ───────────────────────────────────────────────────────────

    #[test]
    fn single_party_no_primary_flag() {
        let reg =
            MpIdRegistry::from_config(&[party("9900001000001", &["NB", "LF"], false)]).unwrap();
        assert_eq!(reg.primary_gln(), "9900001000001");
        assert_eq!(reg.gln_for_role("NB"), Some("9900001000001"));
        assert_eq!(reg.gln_for_role("LF"), Some("9900001000001"));
        assert_eq!(reg.primary_agency(), "293"); // 99-prefix → BDEW
    }

    #[test]
    fn multi_party_primary_selection() {
        let parties = vec![
            party("9900001000001", &["NB"], false),
            party("9900001000002", &["LF"], true), // primary
            party("9900001000003", &["MSB"], false),
        ];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        assert_eq!(reg.primary_gln(), "9900001000002");
        assert_eq!(reg.gln_for_role("NB"), Some("9900001000001"));
        assert_eq!(reg.gln_for_role("LF"), Some("9900001000002"));
        assert_eq!(reg.gln_for_role("MSB"), Some("9900001000003"));
        assert!(reg.is_own_gln("9900001000001"));
        assert!(reg.is_own_gln("9900001000002"));
        assert!(reg.is_own_gln("9900001000003"));
        assert!(!reg.is_own_gln("9900001000099"));
    }

    #[test]
    fn gln_for_role_or_primary_fallback() {
        let reg = MpIdRegistry::from_config(&[party("9900001000001", &["NB"], true)]).unwrap();
        assert_eq!(reg.gln_for_role_or_primary("NB"), "9900001000001");
        assert_eq!(reg.gln_for_role_or_primary("LF"), "9900001000001"); // fallback to primary
    }

    #[test]
    fn case_insensitive_role_lookup() {
        let reg = MpIdRegistry::from_config(&[party("9900001000001", &["nb"], true)]).unwrap();
        assert_eq!(reg.gln_for_role("nb"), Some("9900001000001"));
        assert_eq!(reg.gln_for_role("NB"), Some("9900001000001"));
    }

    // ── Agency derivation ─────────────────────────────────────────────────────

    #[test]
    fn agency_auto_derived_from_gln_prefix() {
        // 99-prefix → BDEW-Codenummer Strom → NAD DE3055 = 293
        let reg = MpIdRegistry::from_config(&[party("9900001000001", &["NB"], true)]).unwrap();
        assert_eq!(reg.primary_agency(), "293");

        // 98-prefix → DVGW-Codenummer Gas → NAD DE3055 = 332
        let reg = MpIdRegistry::from_config(&[party("9800001000001", &["GNB"], true)]).unwrap();
        assert_eq!(reg.primary_agency(), "332");

        // Other 13-digit → GS1 GLN → NAD DE3055 = 9
        let reg = MpIdRegistry::from_config(&[party("4012345000023", &["LF"], true)]).unwrap();
        assert_eq!(reg.primary_agency(), "9");
    }

    #[test]
    fn agency_explicit_override() {
        let mut p = party("9900001000001", &["NB"], true);
        p.agency = Some("9".to_owned()); // force GS1 code despite 99-prefix
        let reg = MpIdRegistry::from_config(&[p]).unwrap();
        assert_eq!(reg.primary_agency(), "9");
        assert_eq!(reg.agency_for_gln("9900001000001"), "9");
        assert_eq!(reg.agency_for_gln("9900001000099"), "293"); // unknown GLN → DEFAULT_AGENCY
    }

    // ── deployment_role_strings ───────────────────────────────────────────────

    #[test]
    fn deployment_role_strings_normalisation() {
        // Gas sub-qualifiers map to Strom-canonical engine role names.
        let parties = vec![
            party("9800001000001", &["GNB", "GMSB"], false), // GNB→NB, GMSB→MSB
            party("9800001000002", &["LFG"], false),         // LFG→LF
            party("9800001000003", &["FNB"], false),         // FNB→UNB
            party("9800001000004", &["MGV"], false),         // excluded (no engine role)
        ];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        let mut roles = reg.deployment_role_strings();
        roles.sort();
        assert_eq!(roles, ["LF", "MSB", "NB", "UNB"]);
    }

    #[test]
    fn deployment_role_strings_excludes_placeholder_roles() {
        let parties = vec![
            party("9900001000001", &["DP"], false),
            party("9900001000002", &["EIV"], false),
            party("9800001000001", &["KN"], false),
            party("4012345000023", &["RB"], false),
        ];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        assert!(reg.deployment_role_strings().is_empty());
    }

    /// ESA gates real PID routing (WiM Teil 2 Kap. 4), so it canonicalises to an
    /// engine role rather than being dropped as a placeholder.
    #[test]
    fn esa_is_an_engine_role() {
        let parties = vec![party("9900001000003", &["ESA"], true)];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        assert_eq!(reg.gln_for_role("ESA"), Some("9900001000003"));
        assert!(reg.deployment_role_strings().contains(&"ESA".to_owned()));
    }

    // ── Known roles ───────────────────────────────────────────────────────────

    #[test]
    fn fnb_and_biko_are_known_roles() {
        let parties = vec![
            party("9900001000001", &["BIKO"], true),
            party("9800001000001", &["FNB"], false),
        ];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        assert_eq!(reg.gln_for_role("BIKO"), Some("9900001000001"));
        assert_eq!(reg.gln_for_role("FNB"), Some("9800001000001"));
        // FNB maps to UNB in engine canonical.
        assert!(reg.deployment_role_strings().contains(&"UNB".to_owned()));
    }

    #[test]
    fn dp_eiv_kn_rb_are_known_but_excluded_from_engine() {
        let parties = vec![
            party("9900001000001", &["DP"], true),
            party("9900001000002", &["EIV"], false),
            party("9800001000001", &["KN"], false),
            party("4012345000023", &["RB"], false),
        ];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        assert_eq!(reg.gln_for_role("DP"), Some("9900001000001"));
        assert_eq!(reg.gln_for_role("KN"), Some("9800001000001"));
        assert_eq!(reg.gln_for_role("RB"), Some("4012345000023"));
        assert!(reg.deployment_role_strings().is_empty());
    }

    // ── §2.13 sparte enforcement ──────────────────────────────────────────────

    #[test]
    fn err_mixed_sparte_roles() {
        let p = party("9900001000001", &["NB", "GNB"], true);
        let err = MpIdRegistry::from_config(&[p]).unwrap_err();
        assert!(
            err.to_string().contains("§2.13"),
            "must reference §2.13: {err}"
        );
    }

    #[test]
    fn err_mixed_sparte_lf_lfg() {
        let p = party("9900001000001", &["LF", "LFG"], true);
        assert!(MpIdRegistry::from_config(&[p]).is_err());
    }

    #[test]
    fn rb_is_sparte_neutral() {
        // RB alongside Strom roles must not trigger §2.13.
        let r = MpIdRegistry::from_config(&[party("9900001000001", &["NB", "RB"], true)]);
        assert!(r.is_ok(), "NB+RB should be ok: {r:?}");

        // RB alongside Gas roles must not trigger §2.13.
        let r = MpIdRegistry::from_config(&[party("9800001000001", &["GNB", "RB"], true)]);
        assert!(r.is_ok(), "GNB+RB should be ok: {r:?}");
    }

    #[test]
    fn separate_strom_gas_entries_ok() {
        let parties = vec![
            party("9900001000001", &["NB", "MSB"], true), // Strom (NB+MSB share BDEW code — valid)
            party("9800001000001", &["GNB", "GMSB"], false), // Gas
            party("9800001000002", &["LFG"], false),      // Gas LF
        ];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        assert_eq!(reg.gln_for_role("NB"), Some("9900001000001"));
        assert_eq!(reg.gln_for_role("GNB"), Some("9800001000001"));
        assert_eq!(reg.gln_for_role("LFG"), Some("9800001000002"));
    }

    // ── Error paths ───────────────────────────────────────────────────────────

    #[test]
    fn err_empty_parties() {
        assert!(MpIdRegistry::from_config(&[]).is_err());
    }

    #[test]
    fn err_invalid_gln() {
        assert!(MpIdRegistry::from_config(&[party("not-a-mp_id", &["NB"], true)]).is_err());
    }

    #[test]
    fn err_duplicate_gln() {
        let parties = vec![
            party("9900001000001", &["NB"], true),
            party("9900001000001", &["LF"], false),
        ];
        assert!(MpIdRegistry::from_config(&parties).is_err());
    }

    #[test]
    fn err_duplicate_role() {
        let parties = vec![
            party("9900001000001", &["NB", "LF"], true),
            party("9900001000002", &["LF", "MSB"], false), // LF in both
        ];
        assert!(MpIdRegistry::from_config(&parties).is_err());
    }

    #[test]
    fn err_multiple_primaries() {
        let parties = vec![
            party("9900001000001", &["NB"], true),
            party("9900001000002", &["LF"], true),
        ];
        assert!(MpIdRegistry::from_config(&parties).is_err());
    }

    #[test]
    fn err_unknown_role() {
        assert!(MpIdRegistry::from_config(&[party("9900001000001", &["INVALID"], true)]).is_err());
    }
}
