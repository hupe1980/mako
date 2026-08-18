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
//! mp_id   = "9900001000001"   # BDEW-Codenummer (99…) → agency "293" auto-derived
//! roles   = ["NB"]
//! primary = true              # storage partition key + default sender
//!
//! # Strom LF — separate BDEW code per role (§2.13):
//! [[party]]
//! mp_id = "9900001000002"
//! roles = ["LF"]
//!
//! # Strom MSB — separate BDEW code (BDEW issues one code per Marktrolle):
//! [[party]]
//! mp_id = "9900001000003"
//! roles = ["MSB"]
//!
//! # Gas GNB — MUST have a different code from all Strom entries (§2.13):
//! [[party]]
//! mp_id = "9800001000001"     # DVGW-Codenummer (98…) → agency "332" auto-derived
//! roles = ["GNB"]
//!
//! # Gas LFG — separate DVGW code per role:
//! [[party]]
//! mp_id = "9800001000002"
//! roles = ["LFG"]
//!
//! # GS1 GLN — agency auto-derived to "9"; override only if the prefix is ambiguous:
//! [[party]]
//! mp_id = "4012345000023"     # GS1 GLN (non-98/99 prefix) → agency "9"
//! roles = ["RB"]              # Registerbetreiber — sparte-neutral
//! ```
//!
//! A role may be declared by its BDEW sub-qualifier (`ANB`, `VNB` for a
//! Netzbetreiber Strom; `GNB`, `LFG`, `GMSB`, `FNB` for Gas). Lookups resolve
//! through the canonical role **within the same Sparte**, so a party declared
//! `VNB` answers a request for `NB` while a Gas `GNB` never does.
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
//! | `KN`  | Kapazitätsnutzer Gas — GaBi Gas capacity booking, placeholder |
//! | `RB`  | Registerbetreiber — MaStR data registry, placeholder |
//!
//! ## Key properties
//!
//! - **AS4 loopback detection** — [`is_own_mp_id`] returns `true` for any GLN
//!   that belongs to this operator, enabling in-process delivery for combined-role
//!   workflows (NB→MSB, GNB→gMSB) regardless of which GLN each role uses.
//!
//! - **EDIFACT sender selection** — [`sender_mp_id_for_orders_pid`] returns the
//!   correct sender GLN for ORDERS messages using a static PID → role table.
//!
//! - **Deployment role derivation** — [`deployment_role_strings`] normalises the
//!   `[[party]]` roles into the strings accepted by `parse_deployment_roles`,
//!   enabling auto-derivation of `--deployment-roles` and `--marktrollen`.
//!
//! [`is_own_mp_id`]: MpIdRegistry::is_own_mp_id
//! [`sender_mp_id_for_orders_pid`]: MpIdRegistry::sender_mp_id_for_orders_pid
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
pub fn sparte_for_role(abbrev: &str) -> Option<RoleSparte> {
    find_role(&abbrev.to_uppercase()).map(|e| e.sparte)
}

// ── NAD agency derivation ─────────────────────────────────────────────────────

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
    primary_mp_id: Arc<str>,
    /// NAD DE3055 agency code for the primary GLN.
    primary_agency: Arc<str>,
    /// All own GLNs — for loopback detection.
    own_mp_ids: HashSet<Arc<str>>,
    /// Normalised role (uppercase) → GLN.
    role_to_gln: HashMap<Box<str>, Arc<str>>,
    /// GLN → NAD DE3055 agency code, for the `[[party]]` entries that set one
    /// explicitly. Read through [`agency_for_mp_id`], which derives the code
    /// from the MP-ID for everything else.
    ///
    /// [`agency_for_mp_id`]: Self::agency_for_mp_id
    mp_id_to_agency: HashMap<Arc<str>, Arc<str>>,
    /// Own GLN → [`RoleSparte`] of the `[[party]]` entry.
    ///
    /// Every `[[party]]` covers exactly one Sparte (BDEW §2.13, enforced in
    /// [`from_config`]), so this is the authoritative Sparte of any interchange
    /// addressed to one of our own MP-IDs — the signal used to decide the Gas-only
    /// CONTRL Empfangsbestätigung obligation (CONTRL AHB 1.0 §2.3.1).
    ///
    /// [`from_config`]: MpIdRegistry::from_config
    mp_id_to_sparte: HashMap<Arc<str>, RoleSparte>,
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
            validate_mp_id(&party.mp_id)?;

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

        let primary_mp_id: Arc<str> = primary.mp_id.as_str().into();
        let primary_agency: Arc<str> = primary
            .agency
            .as_deref()
            .unwrap_or_else(|| derive_agency(&primary.mp_id))
            .into();

        let mut own_mp_ids: HashSet<Arc<str>> = HashSet::new();
        let mut role_to_gln: HashMap<Box<str>, Arc<str>> = HashMap::new();
        let mut mp_id_to_agency: HashMap<Arc<str>, Arc<str>> = HashMap::new();
        let mut mp_id_to_sparte: HashMap<Arc<str>, RoleSparte> = HashMap::new();
        let mut all_roles: Vec<Box<str>> = Vec::new();

        for party in parties {
            let mp_id_arc: Arc<str> = party.mp_id.as_str().into();
            let agency: Arc<str> = party
                .agency
                .as_deref()
                .unwrap_or_else(|| derive_agency(&party.mp_id))
                .into();
            own_mp_ids.insert(Arc::clone(&mp_id_arc));
            mp_id_to_agency.insert(Arc::clone(&mp_id_arc), agency);

            // Aggregate the party's Sparte from its roles. §2.13 (enforced above)
            // guarantees a party never mixes Strom and Gas roles, so the first
            // sector-specific role decides; a party with only sparte-neutral roles
            // (e.g. RB) is `Both`.
            let party_sparte = party
                .roles
                .iter()
                .find_map(|r| match sparte_for_role(r) {
                    Some(RoleSparte::Strom) => Some(RoleSparte::Strom),
                    Some(RoleSparte::Gas) => Some(RoleSparte::Gas),
                    _ => None,
                })
                .unwrap_or(RoleSparte::Both);
            mp_id_to_sparte.insert(Arc::clone(&mp_id_arc), party_sparte);

            for role in &party.roles {
                let key: Box<str> = role.to_uppercase().into_boxed_str();
                all_roles.push(key.clone());
                role_to_gln.insert(key, Arc::clone(&mp_id_arc));
            }
        }
        all_roles.sort_unstable();
        all_roles.dedup();

        Ok(Self {
            primary_mp_id,
            primary_agency,
            own_mp_ids,
            role_to_gln,
            mp_id_to_agency,
            mp_id_to_sparte,
            all_roles,
        })
    }

    // ── Queries ───────────────────────────────────────────────────────────────

    /// Returns the primary GLN (storage partition key / default sender).
    #[must_use]
    pub fn primary_mp_id(&self) -> &str {
        &self.primary_mp_id
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

    /// Returns the [`RoleSparte`] of one of our own MP-IDs.
    ///
    /// Because every `[[party]]` entry covers exactly one Sparte (BDEW §2.13),
    /// the Sparte of an inbound interchange is authoritatively the Sparte of the
    /// own MP-ID it is addressed to (UNB DE0010 receiver). This is the primary
    /// signal for the Gas-only CONTRL Empfangsbestätigung obligation — far more
    /// reliable than PID heuristics, since INVOIC/ORDERS/MSCONS release codes
    /// carry no Sparte prefix and the NAD DE3055 agency code (293 BDEW) is shared
    /// across both sectors in modern MaKo.
    ///
    /// Returns `None` when `mp_id` is not one of our own configured parties, and
    /// [`RoleSparte::Both`] for a sparte-neutral party (only `RB`-type roles).
    #[must_use]
    pub fn sparte_of(&self, mp_id: &str) -> Option<RoleSparte> {
        self.mp_id_to_sparte.get(mp_id).copied()
    }

    /// Returns the GLN for the given BDEW Marktrolle (case-insensitive).
    ///
    /// Resolution is **exact first, then canonical within the same Sparte**.
    /// BDEW sub-qualifiers name the same Marktrolle at finer granularity —
    /// `ANB` and `VNB` are both Netzbetreiber Strom — and `[[party]]` accepts
    /// them, so a caller asking for `"NB"` must find a party that declared
    /// `"VNB"`. Looking up only the literal string returned `None` here, and
    /// [`mp_id_for_role_or_primary`] then silently substituted the primary
    /// MP-ID: in a multi-party deployment that put a *different Marktrolle's*
    /// code into NAD+MS and the UNB sender, which is precisely the identity
    /// confusion BDEW §2.13 exists to prevent.
    ///
    /// The Sparte guard is what keeps the widening safe: `GNB` also
    /// canonicalises to `NB`, but it is a Gas role with its own DVGW code, so a
    /// request for the Strom `"NB"` never resolves to it.
    ///
    /// Returns `None` when no entry declares the role, and — with a warning —
    /// when two entries canonicalise to it with different MP-IDs, since
    /// guessing between them is how a wrong sender reaches the wire.
    ///
    /// [`mp_id_for_role_or_primary`]: MpIdRegistry::mp_id_for_role_or_primary
    #[must_use]
    pub fn mp_id_for_role(&self, role: &str) -> Option<&str> {
        let upper = role.to_uppercase();
        if let Some(exact) = self.role_to_gln.get(upper.as_str()) {
            return Some(exact.as_ref());
        }

        let want = find_role(&upper)?;
        let canonical = want.engine_canonical?;

        let mut found: Option<&str> = None;
        for (declared, mp_id) in &self.role_to_gln {
            let Some(entry) = find_role(declared) else {
                continue;
            };
            if entry.sparte != want.sparte || entry.engine_canonical != Some(canonical) {
                continue;
            }
            match found {
                None => found = Some(mp_id.as_ref()),
                Some(prev) if prev == mp_id.as_ref() => {}
                Some(prev) => {
                    tracing::warn!(
                        requested = %upper,
                        first = %prev,
                        second = %mp_id.as_ref(),
                        "two [[party]] entries resolve to Marktrolle {upper} with different \
                         MP-IDs; refusing to guess the sender. Declare the role explicitly \
                         or set \"sender\" in the payload.",
                    );
                    return None;
                }
            }
        }
        found
    }

    /// Returns the GLN for the given BDEW Marktrolle, or [`primary_mp_id`] as fallback.
    ///
    /// The fallback is correct for a single-`[[party]]` deployment, where the
    /// primary MP-ID *is* every role's code. With several parties it means the
    /// message goes out under some other Marktrolle's code, so it warns.
    ///
    /// [`primary_mp_id`]: MpIdRegistry::primary_mp_id
    #[must_use]
    pub fn mp_id_for_role_or_primary(&self, role: &str) -> &str {
        match self.mp_id_for_role(role) {
            Some(mp_id) => mp_id,
            None => {
                if self.own_mp_ids.len() > 1 {
                    tracing::warn!(
                        role,
                        primary = %self.primary_mp_id,
                        configured_roles = ?self.all_roles,
                        "no [[party]] entry declares Marktrolle {role}; falling back to the \
                         primary MP-ID, which belongs to a different Marktrolle. The outbound \
                         message will carry the wrong sender identity (BDEW §2.13). Add a \
                         [[party]] entry for {role}.",
                    );
                }
                self.primary_mp_id()
            }
        }
    }

    /// Returns the NAD DE3055 agency code for the given Marktpartner-ID.
    ///
    /// A configured `[[party]] agency` override wins; anything else is derived
    /// from the MP-ID itself via `derive_agency`.
    ///
    /// The derivation is what makes this correct for **counterparties**, which
    /// is the main way it is called: the AS4 sender asks for the *recipient's*
    /// agency to fill `<eb:To>/<eb:PartyId type=…>` (AS4-Profil §2.3.1.1), and a
    /// counterparty is by definition absent from our own `[[party]]` list.
    /// Falling back to a fixed `"293"` (BDEW Strom), as this used to, stamped
    /// every Gas counterparty holding a DVGW `98…` code with the BDEW party
    /// type. The receiving MSH resolves its P-Mode from those fields, so the
    /// mismatch is visible to the counterparty, not just internally.
    #[must_use]
    pub fn agency_for_mp_id(&self, mp_id: &str) -> &str {
        self.mp_id_to_agency
            .get(mp_id)
            .map(Arc::as_ref)
            .unwrap_or_else(|| derive_agency(mp_id))
    }

    /// Returns `true` when the given GLN belongs to this operator.
    ///
    /// Used by the AS4 loopback path: a message addressed to an own GLN is
    /// delivered in-process rather than over the network.  Covers ALL own GLNs
    /// so loopback works even when `NB` and `MSB` have different GLNs on the
    /// same `makod` instance.
    #[must_use]
    pub fn is_own_mp_id(&self, mp_id: &str) -> bool {
        self.own_mp_ids.contains(mp_id)
    }

    /// Iterates over all own GLNs (one per `[[party]]` entry).
    pub fn own_mp_ids(&self) -> impl Iterator<Item = &str> {
        self.own_mp_ids.iter().map(Arc::as_ref)
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
    /// Roles with `engine_canonical = None` (`MGV`, `DP`, `EIV`, `KN`, `RB`)
    /// are excluded — they have no active PID routing. `ESA` is **not** among
    /// them: it canonicalises to `ESA`, gates the WiM Teil 2 Wertebestellung
    /// workflows, and has its own `role-esa-strom` build profile.
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
    /// overview.  Falls back to [`primary_mp_id`] when the role is not configured
    /// or the PID is unknown.
    ///
    /// **Ambiguous PIDs** (shared by both Strom and Gas roles with potentially
    /// different GLNs) emit a `warn!` log and fall back to [`primary_mp_id`].
    /// Set `"sender"` explicitly in the ORDERS payload to resolve the ambiguity.
    ///
    /// [`primary_mp_id`]: MpIdRegistry::primary_mp_id
    #[must_use]
    pub fn sender_mp_id_for_orders_pid(&self, pid: u32) -> &str {
        match pid {
            // ── Sperrung / Entsperrung (PIDs 17115–17117) ──────────────────
            // LF initiates Sperrung Strom; LFG initiates Sperrung Gas.
            17115 | 17117 => self.resolve_ambiguous(pid, "LF", "LFG"),
            // NB / GNB issues Entsperrung / MSB-Beauftragung.
            17116 => self.resolve_ambiguous(pid, "NB", "GNB"),

            // ── GPKE Konfigurationseinrichtung (NB → MSB, Teil 3) ───────────
            17134 | 17135 => self.mp_id_for_role_or_primary("NB"),

            // ── WiM Geräteübernahme (NB → MSB / MSBA) ──────────────────────
            17001..=17011 => self.mp_id_for_role_or_primary("NB"),

            // ── Datenabruf / Reklamation (LF → NB/MSB) ─────────────────────
            17102 | 17113 => self.mp_id_for_role_or_primary("LF"),

            // ── Allokationsliste Gas (LF → NB) ──────────────────────────────
            17110 | 17114 => self.mp_id_for_role_or_primary("LF"),

            // ── GPKE Konfigurationsänderung (LF → NB/MSB, Teil 3) ──────────
            17120..=17133 => self.mp_id_for_role_or_primary("LF"),

            // ── Gas Datenabruf (LFG or GNB) ─────────────────────────────────
            17103 | 17104 => self.resolve_ambiguous(pid, "LFG", "GNB"),

            _ => self.primary_mp_id(),
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    fn resolve_ambiguous(&self, pid: u32, role_a: &str, role_b: &str) -> &str {
        match (self.mp_id_for_role(role_a), self.mp_id_for_role(role_b)) {
            (Some(a), Some(b)) if a == b => a,
            (Some(_), Some(_)) => {
                tracing::warn!(
                    pid,
                    role_a,
                    role_b,
                    "ORDERS sender GLN is ambiguous: {role_a} and {role_b} have \
                     different GLNs. Set \"sender\" in the ORDERS payload to resolve. \
                     Falling back to primary_mp_id.",
                );
                self.primary_mp_id()
            }
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => self.primary_mp_id(),
        }
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

/// Validate a BDEW/DVGW MP-ID (13 ASCII digits) or EIC (16 alphanumeric chars).
fn validate_mp_id(mp_id: &str) -> anyhow::Result<()> {
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
mod role_resolution_tests {
    use super::MpIdRegistry;
    use crate::config::PartyConfig;

    fn party(mp_id: &str, roles: &[&str], primary: bool) -> PartyConfig {
        PartyConfig {
            mp_id: mp_id.to_owned(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            primary,
            agency: None,
        }
    }

    /// A grid operator declared with the BDEW sub-qualifier `VNB` must answer a
    /// lookup for `NB`.
    ///
    /// This was the live defect: the lookup matched only the literal string, so
    /// `mp_id_for_role("NB")` returned `None`, and the ORDERS sender fell back
    /// to the primary MP-ID — the *MSB's* code — putting the wrong Marktrolle
    /// into NAD+MS and the UNB sender (BDEW §2.13).
    #[test]
    fn a_sub_qualifier_answers_its_canonical_role() {
        let parties = vec![
            party("9900001000001", &["MSB"], true),
            party("9900001000002", &["VNB"], false),
        ];
        let reg = MpIdRegistry::from_config(&parties).expect("valid config");

        assert_eq!(reg.mp_id_for_role("NB"), Some("9900001000002"));
        assert_eq!(
            reg.sender_mp_id_for_orders_pid(17134),
            "9900001000002",
            "GPKE Konfigurationseinrichtung is sent by the NB, not the MSB"
        );
        assert_eq!(reg.mp_id_for_role("ANB"), Some("9900001000002"));
    }

    /// `GNB` canonicalises to `NB` as well, but it is a Gas role with its own
    /// DVGW code. A Strom `NB` lookup must never resolve to it — that would put
    /// a Gas code on a Strom interchange, the §2.13 violation in the other
    /// direction.
    #[test]
    fn the_canonical_widening_does_not_cross_sparten() {
        let parties = vec![
            party("9900001000001", &["LF"], true),
            party("9800001000001", &["GNB"], false),
        ];
        let reg = MpIdRegistry::from_config(&parties).expect("valid config");

        assert_eq!(reg.mp_id_for_role("GNB"), Some("9800001000001"));
        assert_eq!(
            reg.mp_id_for_role("NB"),
            None,
            "the Gas GNB must not answer a Strom NB lookup"
        );
        assert_eq!(
            reg.mp_id_for_role("LFG"),
            None,
            "the Strom LF must not answer a Gas LFG lookup"
        );
    }

    /// An exact declaration always wins over the canonical widening.
    #[test]
    fn an_exact_role_wins() {
        let parties = vec![party("9900001000001", &["NB"], true)];
        let reg = MpIdRegistry::from_config(&parties).expect("valid config");
        assert_eq!(reg.mp_id_for_role("NB"), Some("9900001000001"));
    }

    /// Two entries canonicalising to the same role with different codes cannot
    /// be resolved. Guessing is how a wrong sender reaches the wire, so the
    /// lookup refuses and the caller falls back visibly.
    #[test]
    fn an_ambiguous_canonical_resolution_refuses() {
        let parties = vec![
            party("9900001000001", &["ANB"], true),
            party("9900001000002", &["VNB"], false),
        ];
        let reg = MpIdRegistry::from_config(&parties).expect("valid config");
        assert_eq!(reg.mp_id_for_role("NB"), None);
    }

    /// A single-party deployment keeps the primary fallback: that MP-ID really
    /// is every role's code there.
    #[test]
    fn a_single_party_deployment_still_falls_back_to_primary() {
        let parties = vec![party("9900001000001", &["NB"], true)];
        let reg = MpIdRegistry::from_config(&parties).expect("valid config");
        assert_eq!(reg.mp_id_for_role_or_primary("BKV"), "9900001000001");
    }
}

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
        assert_eq!(reg.primary_mp_id(), "9900001000001");
        assert_eq!(reg.mp_id_for_role("NB"), Some("9900001000001"));
        assert_eq!(reg.mp_id_for_role("LF"), Some("9900001000001"));
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
        assert_eq!(reg.primary_mp_id(), "9900001000002");
        assert_eq!(reg.mp_id_for_role("NB"), Some("9900001000001"));
        assert_eq!(reg.mp_id_for_role("LF"), Some("9900001000002"));
        assert_eq!(reg.mp_id_for_role("MSB"), Some("9900001000003"));
        assert!(reg.is_own_mp_id("9900001000001"));
        assert!(reg.is_own_mp_id("9900001000002"));
        assert!(reg.is_own_mp_id("9900001000003"));
        assert!(!reg.is_own_mp_id("9900001000099"));
    }

    #[test]
    fn mp_id_for_role_or_primary_fallback() {
        let reg = MpIdRegistry::from_config(&[party("9900001000001", &["NB"], true)]).unwrap();
        assert_eq!(reg.mp_id_for_role_or_primary("NB"), "9900001000001");
        assert_eq!(reg.mp_id_for_role_or_primary("LF"), "9900001000001"); // fallback to primary
    }

    #[test]
    fn case_insensitive_role_lookup() {
        let reg = MpIdRegistry::from_config(&[party("9900001000001", &["nb"], true)]).unwrap();
        assert_eq!(reg.mp_id_for_role("nb"), Some("9900001000001"));
        assert_eq!(reg.mp_id_for_role("NB"), Some("9900001000001"));
    }

    // ── Agency derivation ─────────────────────────────────────────────────────

    #[test]
    fn agency_auto_derived_from_mp_id_prefix() {
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
        assert_eq!(reg.agency_for_mp_id("9900001000001"), "9");
        // Unknown MP-IDs — every counterparty — derive from the prefix.
        assert_eq!(reg.agency_for_mp_id("9900001000099"), "293"); // BDEW Strom
    }

    /// A counterparty's agency must be derived from its own MP-ID.
    ///
    /// # Why this is a test
    ///
    /// The AS4 sender asks for the *recipient's* agency to fill
    /// `<eb:To>/<eb:PartyId type=…>` (AS4-Profil §2.3.1.1), and a counterparty is
    /// never in our own `[[party]]` list — so this method is called with an
    /// unknown MP-ID on every outbound message. It used to answer `"293"` (BDEW
    /// Strom) for all of them, which stamped every Gas counterparty holding a
    /// DVGW `98…` code with the BDEW party type. The receiving MSH resolves its
    /// P-Mode from those fields, so the mismatch is the counterparty's problem
    /// to reject, not an internal detail.
    #[test]
    fn a_counterpartys_agency_comes_from_its_own_mp_id() {
        // A Strom-only operator: no Gas party is configured anywhere.
        let reg = MpIdRegistry::from_config(&[party("9900001000001", &["NB"], true)]).unwrap();

        for (counterparty, expected, what) in [
            ("9812345678901", "332", "DVGW-Codenummer Gas"),
            ("9912345678901", "293", "BDEW-Codenummer Strom"),
            ("4012345000023", "9", "GS1 GLN"),
            ("10XDE-EON-NETZ-C", "ZEW", "EIC"),
        ] {
            assert_eq!(
                reg.agency_for_mp_id(counterparty),
                expected,
                "{counterparty} is a {what}; answering with our own default \
                 would put the wrong PartyId type on every message to them",
            );
        }
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
        assert_eq!(reg.mp_id_for_role("ESA"), Some("9900001000003"));
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
        assert_eq!(reg.mp_id_for_role("BIKO"), Some("9900001000001"));
        assert_eq!(reg.mp_id_for_role("FNB"), Some("9800001000001"));
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
        assert_eq!(reg.mp_id_for_role("DP"), Some("9900001000001"));
        assert_eq!(reg.mp_id_for_role("KN"), Some("9800001000001"));
        assert_eq!(reg.mp_id_for_role("RB"), Some("4012345000023"));
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
        assert_eq!(reg.mp_id_for_role("NB"), Some("9900001000001"));
        assert_eq!(reg.mp_id_for_role("GNB"), Some("9800001000001"));
        assert_eq!(reg.mp_id_for_role("LFG"), Some("9800001000002"));
    }

    #[test]
    fn sparte_of_resolves_own_mp_ids() {
        let parties = vec![
            party("9900001000001", &["NB", "MSB"], true),    // Strom
            party("9800001000001", &["GNB", "GMSB"], false), // Gas
            party("4012345000023", &["RB"], false),          // sparte-neutral (GS1 GLN)
        ];
        let reg = MpIdRegistry::from_config(&parties).unwrap();
        assert_eq!(reg.sparte_of("9900001000001"), Some(RoleSparte::Strom));
        assert_eq!(reg.sparte_of("9800001000001"), Some(RoleSparte::Gas));
        assert_eq!(reg.sparte_of("4012345000023"), Some(RoleSparte::Both));
        // Not one of our own parties → None (falls back to message heuristic).
        assert_eq!(reg.sparte_of("9999999999999"), None);
    }

    // ── Error paths ───────────────────────────────────────────────────────────

    #[test]
    fn err_empty_parties() {
        assert!(MpIdRegistry::from_config(&[]).is_err());
    }

    #[test]
    fn err_invalid_mp_id() {
        assert!(MpIdRegistry::from_config(&[party("not-a-mp_id", &["NB"], true)]).is_err());
    }

    #[test]
    fn err_duplicate_mp_id() {
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
