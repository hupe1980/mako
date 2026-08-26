//! mako's BO4E boundary: the gate every payload crosses, the structural rules
//! the standard states, and the typed columns a stored document is indexed by.
//!
//! BO4E is deliberately permissive — nearly every field is `Option`, even
//! `Betrag.wert` — so it enforces almost nothing on its own. mako's answer is
//! not to distrust it but to divide the labour:
//!
//! | Layer | Module | What it decides |
//! |---|---|---|
//! | **Gate** | [`gate`] | Is this payload the BO it claims, typed, in-schema, and structurally sound? |
//! | **Rules** | [`conformance`] | The two BO4E-stated rules `rubo4e`'s own validators do not cover; the rest delegate to `.validate()`. |
//! | **Columns** | [`columns`] | Which fields of the stored document are queryable, and with what vocabulary. |
//!
//! One call runs the first two:
//!
//! ```
//! use mako_markt::bo4e;
//! use rubo4e::current::Marktlokation;
//!
//! let payload = serde_json::json!({ "marktlokationsId": "51238696781" });
//! let malo: Marktlokation = bo4e::decode(payload).expect("a valid Marktlokation");
//! assert_eq!(malo.marktlokations_id.as_deref(), Some("51238696781"));
//! ```
//!
//! Everything mako *emits* crosses the same rules on the way out
//! ([`ensure_conformant`]), so mako never ships a document it would refuse to
//! accept.

pub mod columns;
pub mod conformance;
pub mod gate;

pub use columns::{
    MAKO_PREISTYP_ATTRIBUT, MaloShadowColumns, MeloShadowColumns, StandorteigenschaftenError,
    ZaehlerShadowColumns, geraet_typ, is_bo4e_preistyp, malo_enum_check_lists,
    melo_enum_check_lists, nelo_enum_check_lists, partner_enum_check_lists, position_preistyp,
};
pub use conformance::Bo4eConformance;
// Re-exported so a caller rendering a rejection does not need its own `rubo4e`
// dependency just to name the failure type.
pub use gate::{
    Bo4eRejection, Bo4eTyped, decode, decode_nested, decode_received, ensure_conformant,
};
pub use rubo4e::validation::ValidationFailure;

use rubo4e::current::Marktlokation;

/// The BO4E schema version every payload mako stores is interpreted under.
///
/// Derived from the linked `rubo4e` rather than written down: mako parses every
/// payload with its own generated types regardless of what a request claims, so
/// the version a row is stamped with is the server's fact, not the client's.
///
/// The column is **provenance, not a migration mechanism** — nothing branches
/// on it. A schema flip means regenerating `rubo4e` and reading existing rows
/// under the new types, which works because unknown fields survive in
/// `_additional`. What the stamp buys is the ability to tell which rows have
/// not been rewritten yet.
pub static SCHEMA_VERSION: std::sync::LazyLock<&'static str> = std::sync::LazyLock::new(|| {
    use rubo4e::Bo4eObject as _;
    Marktlokation::default().schema_version()
});

/// The BO4E schema version, as an owned `String`, for `serde` defaults and
/// row construction.
#[must_use]
pub fn schema_version() -> String {
    (*SCHEMA_VERSION).to_owned()
}

/// The BO4E schema **series** — the `YYYYMM` prefix, without the patch level.
///
/// This is the granularity at which rubo4e exposes a module, and the right key
/// for deciding whether a stored payload is readable: BO4E ships patch releases
/// *inside* a series and every one of them deserializes into the same types.
pub static SCHEMA_SERIES: std::sync::LazyLock<&'static str> = std::sync::LazyLock::new(|| {
    use rubo4e::Bo4eObject as _;
    Marktlokation::default().schema_series()
});

/// Is `stored` a payload version this build can read?
///
/// True for **any** release in the current series. Matching the full triple
/// instead rejects a payload from a producer one BO4E patch ahead that mako
/// reads perfectly — and rejected every payload at all when rubo4e 0.10
/// corrected the wire spelling.
///
/// # The `v` is tolerated on input and never written
///
/// BO4E prefixes its git *tags* with a `v`; the `_version` field inside a
/// payload never has one. A stored value carrying it is read rather than
/// refused; what mako *writes* is always [`SCHEMA_VERSION`].
#[must_use]
pub fn version_is_readable(stored: &str) -> bool {
    stored.strip_prefix('v').unwrap_or(stored).split('.').next() == Some(*SCHEMA_SERIES)
}

#[cfg(test)]
mod schema_version_tests {
    /// The stamp is whatever the linked `rubo4e` generates, so a schema bump
    /// changes it without a source edit — the point of deriving it.
    #[test]
    fn the_schema_version_is_the_linked_crates_own() {
        use rubo4e::Bo4eObject as _;
        assert_eq!(
            super::schema_version(),
            rubo4e::current::Vertrag::default().schema_version(),
            "every BO in a schema series reports the same version"
        );
        // BO4E prefixes its git *tags* with a `v` (`v202607.1.0`); the
        // `_version` field inside a payload never does (`202607.1.0`), and no
        // BO4E schema accepts one that carries it.
        assert!(
            !super::schema_version().starts_with('v'),
            "the payload spelling carries no `v`; only the git tag does"
        );
        assert_eq!(
            super::schema_version().split('.').next(),
            Some(*super::SCHEMA_SERIES),
            "the series is the release's own YYYYMM prefix"
        );
    }

    /// Readability is decided by the **series**, and the old spelling still reads.
    ///
    /// BO4E ships patch releases inside a series and all of them deserialize
    /// into the same types, so matching the full triple would reject a producer
    /// one patch ahead. Rows written before rubo4e 0.10 carry the `v`-prefixed
    /// tag; they stay readable rather than being orphaned by the correction.
    #[test]
    fn the_series_decides_what_is_readable() {
        let series = *super::SCHEMA_SERIES;
        assert!(super::version_is_readable(&super::schema_version()));
        assert!(super::version_is_readable(&format!("{series}.9.9")));
        assert!(super::version_is_readable(&format!("v{series}.0.0")));
        assert!(!super::version_is_readable("202501.0.0"));
        assert!(!super::version_is_readable(""));
    }
}
