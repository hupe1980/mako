//! Tariff store — seeded from PRICAT EDIFACT messages.
//!
//! [`TariffStore`] is the trait that the check engine uses to validate INVOIC
//! unit prices against the NB's published tariff.  [`InMemoryTariffStore`] is
//! the reference implementation used by `invoicd` (seeded from PRICAT 27003)
//! and in unit tests.
//!
//! # Tariff seeding pipeline
//!
//! ```text
//! PRICAT 27003 (NB → LF via AS4)
//!   → makod processes + emits de.mako.process.completed{pid=27003}
//!   → invoicd TariffStore::upsert(nb_gln, entry)
//!   → INVOIC 31001 from same NB GLN arrives
//!   → InvoicCheckEngine::check(&summary, &tariff_store, &config)
//!       → TariffStore::get(nb_gln, billing_date) → Some(TariffEntry)
//!       → compare INVOIC unit_price against TariffEntry::unit_price ± tolerance
//! ```
//!
//! # Temporal lookup
//!
//! Tariff entries are keyed by `(publisher_gln, pricat_pid)`.  Multiple entries
//! for the same GLN + PID can coexist at different effective dates.
//! [`TariffStore::get`] returns the entry whose `valid_from ≤ billing_date` and
//! `valid_to.is_none() || valid_to > billing_date`.  When multiple entries match,
//! the most recent `valid_from` wins.

use std::collections::HashMap;

use crate::amount::EuroAmount;

// ── Domain types ──────────────────────────────────────────────────────────────

/// A single tariff entry extracted from a PRICAT message.
///
/// `TariffEntry` represents one price list item — typically the NNE
/// (Netznutzungsentgelt), Messentgelt, or Ausgleichsenergiepreis published
/// by an NB/MSB/BIKO via PRICAT.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TariffEntry {
    /// GLN of the price list publisher (NB / MSB / BIKO).
    pub publisher_gln: String,
    /// BDEW Prüfidentifikator of the source PRICAT message:
    /// - `27001` — Ausgleichsenergiepreise (BIKO → LF)
    /// - `27002` — MSB service price list (MSB → LF)
    /// - `27003` — NB service price list (NB → LF)
    pub pricat_pid: u32,
    /// Effective start date (YYYYMMDD) of this tariff.
    pub valid_from: String,
    /// Effective end date (YYYYMMDD), exclusive.  `None` = open-ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    /// Charge category identifier (e.g. `"NNE"`, `"MESSUNG"`, `"MMM"`).
    pub charge_category: String,
    /// Published unit price (EUR per kWh or EUR per period, depending on category).
    pub unit_price: EuroAmount,
    /// Tolerance fraction for price comparison.
    ///
    /// `0.01` = 1 %.  When the INVOIC unit price is within this tolerance of
    /// `unit_price`, the tariff check passes.
    pub tolerance: f64,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Read-only access to the LF's tariff database.
///
/// `invoicd` injects a concrete implementation (e.g. [`InMemoryTariffStore`])
/// seeded from incoming PRICAT messages.
pub trait TariffStore {
    /// Find the most recent tariff entry for `publisher_gln` that was effective
    /// on `billing_date` (YYYYMMDD string, e.g. `"20250101"`).
    ///
    /// Returns `None` when no matching entry is found — the check engine treats
    /// a missing tariff as a warning (not a dispute) by default.
    fn get(&self, publisher_gln: &str, billing_date: &str) -> Option<&TariffEntry>;

    /// Return `true` when the store has at least one entry for the given GLN.
    fn has_tariff_for(&self, publisher_gln: &str) -> bool {
        self.get(publisher_gln, "99991231").is_some()
    }
}

// ── InMemoryTariffStore ───────────────────────────────────────────────────────

/// In-memory tariff store backed by a `HashMap<gln, Vec<TariffEntry>>`.
///
/// Entries are stored sorted by `valid_from` descending so that
/// [`TariffStore::get`] can find the most recent valid entry in O(n) per GLN.
///
/// Suitable for `invoicd` (persist entries via PostgreSQL; load into this
/// store at startup) and for unit tests.
#[derive(Debug, Default)]
pub struct InMemoryTariffStore {
    /// Entries grouped by publisher GLN.  Within each list, entries are
    /// stored in descending `valid_from` order so we scan from newest to oldest.
    entries: HashMap<String, Vec<TariffEntry>>,
}

impl InMemoryTariffStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a [`TariffEntry`].
    ///
    /// Entries with the same `publisher_gln + pricat_pid + valid_from` replace
    /// each other (last write wins).
    pub fn insert(&mut self, entry: TariffEntry) {
        let list = self.entries.entry(entry.publisher_gln.clone()).or_default();
        // Replace existing entry with the same primary key or insert.
        if let Some(existing) = list.iter_mut().find(|e| {
            e.pricat_pid == entry.pricat_pid
                && e.valid_from == entry.valid_from
                && e.charge_category == entry.charge_category
        }) {
            *existing = entry;
        } else {
            list.push(entry);
            // Keep sorted newest-first.
            list.sort_by(|a, b| b.valid_from.cmp(&a.valid_from));
        }
    }

    /// Return all entries for a given publisher GLN.
    #[must_use]
    pub fn entries_for(&self, publisher_gln: &str) -> &[TariffEntry] {
        self.entries
            .get(publisher_gln)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Number of distinct GLNs with at least one tariff entry.
    #[must_use]
    pub fn publisher_count(&self) -> usize {
        self.entries.len()
    }
}

impl TariffStore for InMemoryTariffStore {
    fn get(&self, publisher_gln: &str, billing_date: &str) -> Option<&TariffEntry> {
        let entries = self.entries.get(publisher_gln)?;
        // Entries are sorted newest-first.  Find the first entry where
        // valid_from ≤ billing_date AND (valid_to is None OR valid_to > billing_date).
        entries.iter().find(|e| {
            e.valid_from.as_str() <= billing_date
                && e.valid_to.as_deref().is_none_or(|to| to > billing_date)
        })
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(gln: &str, valid_from: &str, valid_to: Option<&str>, price: i64) -> TariffEntry {
        TariffEntry {
            publisher_gln: gln.to_owned(),
            pricat_pid: 27003,
            valid_from: valid_from.to_owned(),
            valid_to: valid_to.map(|s| s.to_owned()),
            charge_category: "NNE".to_owned(),
            unit_price: EuroAmount(price),
            tolerance: 0.01,
        }
    }

    #[test]
    fn insert_and_get_single_entry() {
        let mut store = InMemoryTariffStore::new();
        store.insert(make_entry("9900357000004", "20250101", None, 3_456));

        let entry = store.get("9900357000004", "20250601");
        assert!(entry.is_some());
        assert_eq!(entry.unwrap().unit_price, EuroAmount(3_456));
    }

    #[test]
    fn get_unknown_gln_returns_none() {
        let store = InMemoryTariffStore::new();
        assert!(store.get("unknown", "20250601").is_none());
    }

    #[test]
    fn get_before_valid_from_returns_none() {
        let mut store = InMemoryTariffStore::new();
        store.insert(make_entry("9900357000004", "20250101", None, 3_456));
        // Query for a date before the tariff is effective.
        assert!(store.get("9900357000004", "20241231").is_none());
    }

    #[test]
    fn get_after_valid_to_returns_none() {
        let mut store = InMemoryTariffStore::new();
        store.insert(make_entry(
            "9900357000004",
            "20250101",
            Some("20251231"),
            3_456,
        ));
        // Query for a date after validity ended.
        assert!(store.get("9900357000004", "20260101").is_none());
    }

    #[test]
    fn get_returns_most_recent_effective_entry() {
        let mut store = InMemoryTariffStore::new();
        // Old tariff: 2024 rate.
        store.insert(make_entry(
            "9900357000004",
            "20240101",
            Some("20241231"),
            3_000,
        ));
        // New tariff: 2025 rate.
        store.insert(make_entry("9900357000004", "20250101", None, 3_456));

        // Query mid-2025 — should return the 2025 entry.
        let entry = store.get("9900357000004", "20250601").unwrap();
        assert_eq!(entry.unit_price, EuroAmount(3_456));
        assert_eq!(entry.valid_from, "20250101");
    }

    #[test]
    fn get_returns_old_entry_within_its_validity() {
        let mut store = InMemoryTariffStore::new();
        store.insert(make_entry(
            "9900357000004",
            "20240101",
            Some("20241231"),
            3_000,
        ));
        store.insert(make_entry("9900357000004", "20250101", None, 3_456));

        // Query mid-2024 — should return the 2024 entry.
        let entry = store.get("9900357000004", "20240601").unwrap();
        assert_eq!(entry.unit_price, EuroAmount(3_000));
    }

    #[test]
    fn has_tariff_for_present_and_absent() {
        let mut store = InMemoryTariffStore::new();
        store.insert(make_entry("9900357000004", "20250101", None, 3_456));
        assert!(store.has_tariff_for("9900357000004"));
        assert!(!store.has_tariff_for("unknown"));
    }

    #[test]
    fn insert_replaces_same_primary_key() {
        let mut store = InMemoryTariffStore::new();
        store.insert(make_entry("9900357000004", "20250101", None, 3_456));
        // Upsert same key with updated price.
        store.insert(make_entry("9900357000004", "20250101", None, 4_000));
        let entry = store.get("9900357000004", "20250601").unwrap();
        // Should return updated price.
        assert_eq!(entry.unit_price, EuroAmount(4_000));
        assert_eq!(store.entries_for("9900357000004").len(), 1);
    }
}
