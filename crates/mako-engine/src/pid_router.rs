//! PID-to-workflow routing table.
//!
//! Every inbound EDIFACT message carries a `Prüfidentifikator` (PID) that
//! identifies the MaKo process family and operation. The `PidRouter` maps
//! numeric PID values to workflow names, enabling dispatchers to instantiate
//! the correct [`Workflow`] implementation without ad-hoc `match` chains.
//!
//! # Mutability contract — build-time only
//!
//! `PidRouter` uses `&mut self` for all registrations. In normal engine usage
//! the router is populated **once** during `EngineBuilder::build()` and is
//! subsequently sealed inside `EngineContext` behind a shared `&PidRouter`
//! reference. There is no runtime mutation path — all PIDs must be registered
//! before the engine starts serving messages.
//!
//! This is intentional: mutation after startup would race with concurrent
//! dispatch calls and require a `RwLock`. The read-only runtime path is
//! therefore always lock-free.
//!
//! # BDEW PID ranges (incomplete — register all PIDs for your process families)
//!
//! | Range | Process family |
//! |---|---|
//! | 11001–11099 | WiM Gerätewechsel (UTILMD) |
//! | 13003        | MABIS Bilanzkreisabrechnung (MSCONS) |
//! | 13002–13028  | Messwerte Gas/Strom/Redispatch (MSCONS) — fragmented across GaBi Gas, Redispatch, GPKE support |
//! | 17001–17011 | WiM MSB commissioning (ORDERS) |
//! | 17101–17135 | WiM Stammdaten / Konfiguration (ORDERS) |
//! | 31001–31002, 31004–31008 | GPKE Netznutzungsabrechnung / MMM-Rechnung (INVOIC) |
//! | 31003, 31009 | WiM-Rechnung / MSB-Rechnung (INVOIC) — WiM domain |
//! | 31010 | Kapazitätsrechnung (INVOIC) — Kapazitätsabrechnung Ausspeisepunkte Gas |
//! | 31011 | Rechnung sonstige Leistung (INVOIC) — AWH Sperrprozesse Gas |
//! | 33001–33004  | REMADV Bestätigung/Abweisung — paired with INVOIC workflows |
//! | 37000–37006  | PARTIN Kommunikationsdaten Strom (GPKE Teil 4) |
//! | 37008–37014  | PARTIN Kommunikationsdaten Gas (GeLi Gas 2.0) |
//! | 39000–39001 | ORDCHG Stornierung Sperr-/Entsperrauftrag (AWH Sperrprozesse Gas) |
//! | 39002 | ORDCHG Stornierung Bestellung (WiM Strom Teil 2) |
//! | 44001–44018 | GeLi Gas Lieferantenwechsel (UTILMD G) |
//! | 55001–55018 | GPKE Lieferantenwechsel / Kündigung (UTILMD Strom) |
//! | 55555 | GPKE Teil 4 — Anfrage Daten der individuellen Bestellung (UTILMD Strom) |
//!
//! # Usage
//!
//! ```rust
//! use mako_engine::pid_router::PidRouter;
//!
//! let mut router = PidRouter::new();
//! router.register(55001, "GpkeSupplierChange");
//! router.register(55002, "GpkeSupplierChange"); // Same workflow, different step
//!
//! assert_eq!(router.route(55001), Some("GpkeSupplierChange"));
//! assert_eq!(router.route(99999), None);
//! assert_eq!(router.len(), 2);
//! ```
//!
//! [`Workflow`]: crate::workflow::Workflow

use std::collections::HashMap;

use crate::types::Sparte;

// ── PidRouter ─────────────────────────────────────────────────────────────────

/// A static mapping from `Prüfidentifikator` (PID) values to workflow names.
///
/// Register all PIDs your platform handles before starting the engine. At
/// runtime, call [`route`] to look up the workflow name for an inbound PID.
///
/// The workflow name matches [`WorkflowId::name`] — use it to select the
/// correct `Workflow` implementation in your message dispatcher.
///
/// # Mutability contract
///
/// `PidRouter` exposes a `&mut self` API for registrations (`register`). In
/// the engine this mutability is exercised **only** during
/// [`EngineBuilder::build`] — after that the router is owned by
/// [`EngineContext`] and only shared references are available at runtime.
/// There is no way to mutate the router from an async dispatch handler.
///
/// Duplicate registrations silently replace the previous mapping; the last
/// call wins. Use `cargo xtask validate-pruefids` to detect PID conflicts
/// between modules before they reach production.
///
/// # Building a complete router
///
/// In your `main` or integration module, register every PID that the platform
/// must handle. PIDs not registered will return `None` from [`route`], causing
/// the dispatcher to dead-letter the message cleanly.
///
/// ```rust
/// use mako_engine::pid_router::PidRouter;
///
/// fn build_router() -> PidRouter {
///     let mut r = PidRouter::new();
///     // GPKE Lieferantenwechsel (BK6-22-024) — UTILMD
///     r.register(55001, "GpkeSupplierChange");
///     r.register(55002, "GpkeSupplierChange");
///     r.register(55003, "GpkeSupplierChange");
///     r.register(55004, "GpkeSupplierChange");
///     r
/// }
/// ```
///
/// [`route`]: PidRouter::route
/// [`WorkflowId::name`]: crate::version::WorkflowId::name
/// [`EngineBuilder::build`]: crate::builder::EngineBuilder::build
/// [`EngineContext`]: crate::builder::EngineContext
#[derive(Debug, Default, Clone)]
pub struct PidRouter {
    table: HashMap<u32, Box<str>>,
    /// Commodity-qualified routing table: `(pid, Sparte) → workflow_name`.
    ///
    /// Checked first by [`route_with_sparte`]; falls back to the unambiguous
    /// [`table`] when no commodity-specific entry exists.
    ///
    /// Use this for PIDs shared between Strom and Gas process families where
    /// the APERAK Frist differs:
    ///
    /// | PID   | Strom workflow  | Gas workflow      |
    /// |-------|-----------------|-------------------|
    /// | 23001 | `wim-insrpt`    | `wim-gas-insrpt`  |
    /// | 23003 | `wim-insrpt`    | `wim-gas-insrpt`  |
    /// | 23004 | `wim-insrpt`    | `wim-gas-insrpt`  |
    /// | 23008 | `wim-insrpt`    | `wim-gas-insrpt`  |
    ///
    /// [`route_with_sparte`]: PidRouter::route_with_sparte
    /// [`table`]: PidRouter::table
    commodity_table: HashMap<(u32, Sparte), Box<str>>,
    /// Tracks which module registered each PID for conflict detection.
    ///
    /// Populated by [`register_with_module`]; used to produce actionable
    /// panic messages when two modules register the same PID to different workflows.
    ///
    /// [`register_with_module`]: PidRouter::register_with_module
    registered_by: HashMap<u32, Box<str>>,
}

impl PidRouter {
    /// Create an empty router.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `pid` as routing to `workflow_name`.
    ///
    /// If `pid` was already registered, the previous mapping is silently
    /// replaced. Call this only at build time (via [`EngineModule::register_pids`]);
    /// the method is `&mut self` to prevent accidental runtime mutation once the
    /// router is sealed inside [`EngineContext`].
    ///
    /// Accepts any string — `&'static str`, `String`, or `Box<str>`.
    ///
    /// For conflict-detected registration (preferred in multi-module builds),
    /// use [`register_with_module`] instead.
    ///
    /// [`EngineModule::register_pids`]: crate::builder::EngineModule::register_pids
    /// [`EngineContext`]: crate::builder::EngineContext
    /// [`register_with_module`]: PidRouter::register_with_module
    pub fn register(&mut self, pid: u32, workflow_name: impl Into<Box<str>>) {
        let wf = workflow_name.into();
        self.table.insert(pid, wf);
    }

    /// Register `pid` → `workflow_name` with module-attribution conflict detection.
    ///
    /// # Panics
    ///
    /// Panics at **build time** (before the engine starts) if `pid` is already
    /// registered to a *different* workflow name by a *different* module. Two
    /// modules registering the same PID to the **same** workflow are silently
    /// accepted (idempotent).
    ///
    /// Use [`DeploymentRoles`] to prevent two modules from registering the same
    /// PID when only one role is active:
    ///
    /// ```rust,ignore
    /// // Both GPKE (NB role) and WiM (nMSB role) register 19001 → different workflows.
    /// // Set explicit roles so only one module's conditional block fires:
    /// use mako_engine::marktrolle::{DeploymentRoles, Marktrolle};
    /// let roles = DeploymentRoles::from_roles([Marktrolle::Nb]);
    /// // Now only GPKE registers 19001 → "gpke-konfiguration".
    /// ```
    ///
    /// [`DeploymentRoles`]: crate::marktrolle::DeploymentRoles
    pub fn register_with_module(
        &mut self,
        pid: u32,
        workflow_name: impl Into<Box<str>>,
        module: &str,
    ) {
        let wf = workflow_name.into();
        if let Some(existing_wf) = self.table.get(&pid)
            && *existing_wf != wf
        {
            let existing_mod = self
                .registered_by
                .get(&pid)
                .map_or("<unknown>", Box::as_ref);
            panic!(
                "PID {pid} routing conflict:\n  \
                     module '{module}' tried to register PID {pid} → '{wf}'\n  \
                     but it was already registered → '{existing_wf}' by module '{existing_mod}'\n  \
                     Hint: use DeploymentRoles to prevent conflicting modules from \
                     both registering shared PIDs (e.g. 19001/19002 are claimed by \
                     gpke-konfiguration for NB role and wim-geraeteubernahme for nMSB role).\n  \
                     Set EngineBuilder::with_deployment_roles(DeploymentRoles::nb()) to keep \
                     only the NB-role registration."
            );
        }
        self.table.insert(pid, wf);
        self.registered_by.insert(pid, module.into());
    }

    /// Register `pid` → `workflow_name` for a specific commodity ([`Sparte`]).
    ///
    /// Use this for PIDs that map to **different workflows depending on whether
    /// the message concerns electricity (Strom) or gas (Gas)**. At runtime call
    /// [`route_with_sparte`] to prefer the commodity-specific entry over the
    /// unambiguous fallback registered via [`register`].
    ///
    /// # INSRPT shared PIDs (23001/23003/23004/23008)
    ///
    /// These PIDs appear in both WiM Strom (5 WT) and WiM Gas (10 WT) AHBs:
    ///
    /// ```rust,ignore
    /// // In WimModule (Strom):
    /// router.register_with_sparte(23001, Sparte::Strom, "wim-insrpt");
    ///
    /// // In WimGasModule (Gas):
    /// router.register_with_sparte(23001, Sparte::Gas, "wim-gas-insrpt");
    /// ```
    ///
    /// [`route_with_sparte`]: PidRouter::route_with_sparte
    /// [`register`]: PidRouter::register
    pub fn register_with_sparte(
        &mut self,
        pid: u32,
        sparte: Sparte,
        workflow_name: impl Into<Box<str>>,
    ) {
        self.commodity_table
            .insert((pid, sparte), workflow_name.into());
    }

    /// Look up the workflow name for `pid`, preferring the commodity-qualified
    /// entry for `sparte` over the unambiguous fallback.
    ///
    /// Resolution order:
    /// 1. `commodity_table[(pid, sparte)]` — registered via [`register_with_sparte`]
    /// 2. `table[pid]` — registered via [`register`] (unambiguous fallback)
    ///
    /// Returns `None` when neither table has an entry for `pid`.
    ///
    /// # INSRPT routing example
    ///
    /// ```rust
    /// use mako_engine::pid_router::PidRouter;
    /// use mako_engine::types::Sparte;
    ///
    /// let mut r = PidRouter::new();
    /// r.register(23001, "wim-insrpt");                            // Strom fallback
    /// r.register_with_sparte(23001, Sparte::Strom, "wim-insrpt");
    /// r.register_with_sparte(23001, Sparte::Gas,   "wim-gas-insrpt");
    ///
    /// assert_eq!(r.route_with_sparte(23001, Sparte::Strom), Some("wim-insrpt"));
    /// assert_eq!(r.route_with_sparte(23001, Sparte::Gas),   Some("wim-gas-insrpt"));
    /// assert_eq!(r.route(23001),                             Some("wim-insrpt"));
    /// ```
    ///
    /// [`register_with_sparte`]: PidRouter::register_with_sparte
    /// [`register`]: PidRouter::register
    #[must_use]
    pub fn route_with_sparte(&self, pid: u32, sparte: Sparte) -> Option<&str> {
        self.commodity_table
            .get(&(pid, sparte))
            .or_else(|| self.table.get(&pid))
            .map(Box::as_ref)
    }

    /// Look up the workflow name for `pid`.
    ///
    /// Returns `None` when `pid` has not been registered. The caller should
    /// dead-letter the message and return an appropriate error to the sender
    /// rather than panicking.
    #[must_use]
    pub fn route(&self, pid: u32) -> Option<&str> {
        self.table.get(&pid).map(Box::as_ref)
    }

    /// Return an iterator over all registered PID values.
    ///
    /// Useful for validation (e.g. comparing against PIDs declared in
    /// AHB profile JSON files to detect missing workflow implementations).
    pub fn registered_pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.table.keys().copied()
    }

    /// Return the number of registered PID mappings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Return `true` when no PIDs have been registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_registered_pid() {
        let mut r = PidRouter::new();
        r.register(55001, "GpkeSupplierChange");
        assert_eq!(r.route(55001), Some("GpkeSupplierChange"));
    }
    #[test]
    fn route_unregistered_pid_returns_none() {
        let r = PidRouter::new();
        assert_eq!(r.route(55001), None);
    }

    #[test]
    fn register_overwrites_previous_mapping() {
        let mut r = PidRouter::new();
        r.register(55001, "OldWorkflow");
        r.register(55001, "NewWorkflow");
        assert_eq!(r.route(55001), Some("NewWorkflow"));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn registered_pids_covers_all_entries() {
        let mut r = PidRouter::new();
        r.register(55001, "A");
        r.register(55002, "B");
        r.register(11001, "C");

        let mut pids: Vec<u32> = r.registered_pids().collect();
        pids.sort_unstable();
        assert_eq!(pids, [11001, 55001, 55002]);
    }

    #[test]
    fn multiple_pids_same_workflow() {
        let mut r = PidRouter::new();
        r.register(55001, "GpkeSupplierChange");
        r.register(55002, "GpkeSupplierChange");
        r.register(55003, "GpkeSupplierChange");

        assert_eq!(r.len(), 3);
        for pid in [55001, 55002, 55003] {
            assert_eq!(r.route(pid), Some("GpkeSupplierChange"));
        }
    }

    #[test]
    fn is_empty_and_len() {
        let mut r = PidRouter::new();
        assert!(r.is_empty());
        r.register(55001, "W");
        assert!(!r.is_empty());
        assert_eq!(r.len(), 1);
    }

    // ── Commodity-aware routing ───────────────────────────────────────────────

    #[test]
    fn route_with_sparte_prefers_commodity_entry() {
        use crate::types::Sparte;
        let mut r = PidRouter::new();
        r.register(23001, "wim-insrpt"); // unambiguous fallback
        r.register_with_sparte(23001, Sparte::Strom, "wim-insrpt");
        r.register_with_sparte(23001, Sparte::Gas, "wim-gas-insrpt");

        assert_eq!(
            r.route_with_sparte(23001, Sparte::Strom),
            Some("wim-insrpt")
        );
        assert_eq!(
            r.route_with_sparte(23001, Sparte::Gas),
            Some("wim-gas-insrpt")
        );
        // Unambiguous route() is unaffected by commodity table.
        assert_eq!(r.route(23001), Some("wim-insrpt"));
    }

    #[test]
    fn route_with_sparte_falls_back_to_unambiguous() {
        use crate::types::Sparte;
        let mut r = PidRouter::new();
        // No commodity entry — only unambiguous.
        r.register(55001, "GpkeSupplierChange");

        assert_eq!(
            r.route_with_sparte(55001, Sparte::Strom),
            Some("GpkeSupplierChange")
        );
        assert_eq!(
            r.route_with_sparte(55001, Sparte::Gas),
            Some("GpkeSupplierChange")
        );
    }

    #[test]
    fn route_with_sparte_returns_none_for_unregistered() {
        use crate::types::Sparte;
        let r = PidRouter::new();
        assert_eq!(r.route_with_sparte(23001, Sparte::Strom), None);
        assert_eq!(r.route_with_sparte(23001, Sparte::Gas), None);
    }

    #[test]
    fn route_with_sparte_gas_only_deployment() {
        use crate::types::Sparte;
        // Gas-standalone: only Gas entries; no WimModule Strom fallback.
        let mut r = PidRouter::new();
        r.register(23001, "wim-gas-insrpt"); // unambiguous (Gas-standalone)
        r.register_with_sparte(23001, Sparte::Gas, "wim-gas-insrpt");

        assert_eq!(
            r.route_with_sparte(23001, Sparte::Gas),
            Some("wim-gas-insrpt")
        );
        // Strom falls back to unambiguous (no Strom-specific entry exists).
        assert_eq!(
            r.route_with_sparte(23001, Sparte::Strom),
            Some("wim-gas-insrpt")
        );
    }

    #[test]
    fn route_with_sparte_combined_deployment_all_shared_insrpt_pids() {
        use crate::types::Sparte;
        let mut r = PidRouter::new();
        // WimModule registers shared PIDs as Strom:
        for pid in [23001_u32, 23003, 23004, 23008] {
            r.register(pid, "wim-insrpt");
            r.register_with_sparte(pid, Sparte::Strom, "wim-insrpt");
        }
        // WimGasModule registers shared PIDs as Gas:
        for pid in [23001_u32, 23003, 23004, 23008] {
            r.register_with_sparte(pid, Sparte::Gas, "wim-gas-insrpt");
        }

        for pid in [23001_u32, 23003, 23004, 23008] {
            assert_eq!(
                r.route_with_sparte(pid, Sparte::Strom),
                Some("wim-insrpt"),
                "PID {pid} Strom should route to wim-insrpt"
            );
            assert_eq!(
                r.route_with_sparte(pid, Sparte::Gas),
                Some("wim-gas-insrpt"),
                "PID {pid} Gas should route to wim-gas-insrpt"
            );
        }
    }
}
