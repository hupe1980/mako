//! Price-sheet store — seeded from PRICAT 27003 EDIFACT messages.
//!
//! [`PreisblattStore`] is the trait the check engine uses to validate INVOIC
//! unit prices against the NB's published [`PreisblattNetznutzung`].
//! [`InMemoryPreisblattStore`] is the reference implementation used by
//! `invoicd` (seeded at startup from `marktd`'s price-sheet API) and in tests.
//!
//! # Price-sheet seeding pipeline
//!
//! ```text
//! PRICAT 27003 (NB → LF via AS4)
//!   → makod processes + emits de.mako.process.completed{pid=27003}
//!   → marktd: PUT /api/v1/preisblaetter/{nb_mp_id}  (persisted to PostgreSQL)
//!   → invoicd: GET /api/v1/preisblaetter/{nb_mp_id}?date={billing_date}
//!       → MdmdPreisblattClient (1h LRU cache, circuit breaker)
//!   → InvoicCheckEngine::check(&rechnung, &preisblatt_store, &config)
//!       → PreisblattStore::get(nb_mp_id, billing_date) → Some(PreisblattNetznutzung)
//!       → compare INVOIC einzelpreis against preisblatt.preispositionen[*].preisstaffeln[*].preis ± tolerance
//! ```
//!
//! # Temporal lookup
//!
//! Entries are keyed by `nb_mp_id`.  Multiple `PreisblattNetznutzung` records
//! for the same GLN can coexist at different validity periods.
//! [`PreisblattStore::get`] returns the sheet whose
//! `gueltigkeit.startdatum ≤ billing_date < gueltigkeit.enddatum`
//! (open-ended `enddatum` is treated as ∞).
//! When multiple sheets match, the one with the most recent `startdatum` wins.

use std::collections::HashMap;

use rubo4e::current::PreisblattNetznutzung;

// ── Trait ─────────────────────────────────────────────────────────────────────

/// Read-only access to the NB price-sheet database.
///
/// `invoicd` injects a concrete implementation (e.g. [`InMemoryPreisblattStore`]
/// seeded from `marktd`'s price-sheet API) at check time.
pub trait PreisblattStore {
    /// Find the most recent `PreisblattNetznutzung` for `nb_mp_id` that was
    /// effective on `billing_date`.
    ///
    /// Returns `None` when no matching price sheet is found — the check engine
    /// treats a missing price sheet as a warning (never a hard dispute) by
    /// default.
    fn get(&self, nb_mp_id: &str, billing_date: time::Date) -> Option<&PreisblattNetznutzung>;

    /// Return `true` when the store has at least one entry for the given GLN.
    fn has_preisblatt_for(&self, nb_mp_id: &str) -> bool {
        self.get(nb_mp_id, time::macros::date!(9999 - 12 - 31))
            .is_some()
    }
}

// ── InMemoryPreisblattStore ───────────────────────────────────────────────────

/// In-memory price-sheet store backed by a `HashMap<gln, Vec<PreisblattNetznutzung>>`.
///
/// Entries are stored sorted by `gueltigkeit.startdatum` descending so that
/// [`PreisblattStore::get`] can find the most recent valid sheet in O(n) per GLN.
///
/// Suitable for `invoicd` (cache populated from `marktd`'s price-sheet API at
/// startup) and for unit tests.
#[derive(Debug, Default)]
pub struct InMemoryPreisblattStore {
    inner: HashMap<String, Vec<PreisblattNetznutzung>>,
}

impl InMemoryPreisblattStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Insert a `PreisblattNetznutzung` for the given NB GLN.
    ///
    /// After insertion the list is **not** automatically re-sorted; call
    /// [`sort`][Self::sort] once all entries are loaded if insertion order
    /// matters.
    pub fn insert(&mut self, nb_mp_id: String, sheet: PreisblattNetznutzung) {
        self.inner.entry(nb_mp_id).or_default().push(sheet);
    }

    /// Sort all entry lists by `gueltigkeit.startdatum` descending so that
    /// [`PreisblattStore::get`] returns the most-recently-valid sheet first.
    ///
    /// Uses the `validity()` convenience method from rubo4e v0.5 — direct
    /// `time::Date` comparison with no intermediate string allocation.
    pub fn sort(&mut self) {
        for sheets in self.inner.values_mut() {
            sheets.sort_by(|a, b| {
                let a_start = a.validity().map(|(s, _)| s);
                let b_start = b.validity().map(|(s, _)| s);
                b_start.cmp(&a_start)
            });
        }
    }
}

impl PreisblattStore for InMemoryPreisblattStore {
    fn get(&self, nb_mp_id: &str, billing_date: time::Date) -> Option<&PreisblattNetznutzung> {
        let sheets = self.inner.get(nb_mp_id)?;
        sheets.iter().find(|s| sheet_is_valid(s, billing_date))
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Return `true` when `billing_date` falls within the sheet's validity window.
///
/// Uses the `validity()` convenience method from rubo4e v0.5 for direct
/// `time::Date` comparison — no string allocation or formatting needed.
///
/// Window: `startdatum <= billing_date` AND (`enddatum` absent OR `billing_date < enddatum`)
/// A missing `gueltigkeit` or missing `startdatum` means open-started (always valid from the past).
/// A missing `enddatum` means open-ended (valid until replaced).
fn sheet_is_valid(sheet: &PreisblattNetznutzung, billing_date: time::Date) -> bool {
    match sheet.validity() {
        None => true,
        Some((start, None)) => billing_date >= start,
        Some((start, Some(end))) => billing_date >= start && billing_date < end,
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use rubo4e::current::{PreisblattNetznutzung, Zeitraum};
    use time::macros::date;

    use super::*;

    fn make_sheet(start: Option<time::Date>, end: Option<time::Date>) -> PreisblattNetznutzung {
        let gueltigkeit = if start.is_some() || end.is_some() {
            Some(Zeitraum {
                startdatum: start,
                enddatum: end,
                ..Default::default()
            })
        } else {
            None
        };
        PreisblattNetznutzung {
            gueltigkeit,
            ..Default::default()
        }
    }

    #[test]
    fn test_store_get_finds_valid_sheet() {
        let mut store = InMemoryPreisblattStore::new();
        let sheet = make_sheet(Some(date!(2025 - 01 - 01)), Some(date!(2026 - 01 - 01)));
        store.insert("9900000000001".to_owned(), sheet);

        assert!(store.get("9900000000001", date!(2025 - 06 - 01)).is_some());
        assert!(store.get("9900000000001", date!(2024 - 12 - 31)).is_none());
        assert!(store.get("9900000000001", date!(2026 - 01 - 01)).is_none()); // exclusive end
        assert!(store.get("9900000000999", date!(2025 - 06 - 01)).is_none()); // unknown mp_id
    }

    #[test]
    fn test_store_open_ended_sheet() {
        let mut store = InMemoryPreisblattStore::new();
        let sheet = make_sheet(Some(date!(2025 - 01 - 01)), None);
        store.insert("9900000000002".to_owned(), sheet);
        assert!(store.get("9900000000002", date!(2099 - 12 - 30)).is_some());
    }

    #[test]
    fn test_has_preisblatt_for() {
        let mut store = InMemoryPreisblattStore::new();
        let sheet = make_sheet(None, None);
        store.insert("9900000000003".to_owned(), sheet);
        assert!(store.has_preisblatt_for("9900000000003"));
        assert!(!store.has_preisblatt_for("9900000000999"));
    }
}
