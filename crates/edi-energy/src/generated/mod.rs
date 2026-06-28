// @generated — do not edit by hand; run `cargo xtask codegen` to regenerate
//
// Generated profiles: 40
#![allow(clippy::used_underscore_binding)]

pub(super) mod ahb_helpers;

#[cfg(feature = "aperak")]
mod aperak_fv20251001;
#[cfg(feature = "aperak")]
mod aperak_fv20261001;
#[cfg(feature = "comdis")]
mod comdis_fv20251001;
#[cfg(feature = "comdis")]
mod comdis_fv20261001;
// Archived profile — excluded from the default build.
// Enable the `contrl-archive` or `archive` Cargo feature to include.
#[cfg(any(feature = "contrl-archive", feature = "archive"))]
mod contrl_fv20251001;
#[cfg(feature = "contrl")]
mod contrl_fv20260101;
#[cfg(feature = "iftsta")]
mod iftsta_fv20251001;
#[cfg(feature = "iftsta")]
mod iftsta_fv20261001;
// Archived profile — excluded from the default build.
// Enable the `insrpt-archive` or `archive` Cargo feature to include.
#[cfg(any(feature = "insrpt-archive", feature = "archive"))]
mod insrpt_fv20211001;
#[cfg(feature = "insrpt")]
mod insrpt_fv20260101;
#[cfg(feature = "invoic")]
mod invoic_fv20251001;
#[cfg(feature = "invoic")]
mod invoic_fv20260401;
// Archived profile — excluded from the default build.
// Enable the `mscons-archive` or `archive` Cargo feature to include.
#[cfg(any(feature = "mscons-archive", feature = "archive"))]
mod mscons_fv20240401;
#[cfg(feature = "mscons")]
mod mscons_fv20251001;
#[cfg(feature = "mscons")]
mod mscons_fv20261001;
#[cfg(feature = "ordchg")]
mod ordchg_fv20241001;
#[cfg(feature = "ordchg")]
mod ordchg_fv20260401;
#[cfg(feature = "orders")]
mod orders_fv20251001;
#[cfg(feature = "orders")]
mod orders_fv20260401;
#[cfg(feature = "ordrsp")]
mod ordrsp_fv20251001;
#[cfg(feature = "ordrsp")]
mod ordrsp_fv20260401;
#[cfg(feature = "partin")]
mod partin_fv20251001;
#[cfg(feature = "partin")]
mod partin_fv20260401;
#[cfg(feature = "pricat")]
mod pricat_fv20250401;
#[cfg(feature = "pricat")]
mod pricat_fv20260401;
#[cfg(feature = "quotes")]
mod quotes_fv20250401;
#[cfg(feature = "quotes")]
mod quotes_fv20260401;
#[cfg(feature = "remadv")]
mod remadv_fv20251001;
#[cfg(feature = "remadv")]
mod remadv_fv20260401;
#[cfg(feature = "reqote")]
mod reqote_fv20250401;
#[cfg(feature = "reqote")]
mod reqote_fv20260401;
#[cfg(feature = "utilmd")]
mod utilmd_fv20241001;
#[cfg(feature = "utilmd")]
mod utilmd_fv20241001_gas;
#[cfg(feature = "utilmd")]
mod utilmd_fv20250606;
#[cfg(feature = "utilmd")]
mod utilmd_fv20251001;
#[cfg(feature = "utilmd")]
mod utilmd_fv20251001_gas;
#[cfg(feature = "utilmd")]
mod utilmd_fv20261001;
#[cfg(feature = "utilmd")]
mod utilmd_fv20261001_gas;
#[cfg(feature = "utilts")]
mod utilts_fv20241001;
#[cfg(feature = "utilts")]
mod utilts_fv20260401;

use crate::registry::Profile;

/// Register all generated profiles into `profiles`.
pub(crate) fn register_profiles(_profiles: &mut Vec<&'static dyn Profile>) {
    #[cfg(feature = "aperak")]
    _profiles.push(&aperak_fv20251001::PROFILE);
    #[cfg(feature = "aperak")]
    _profiles.push(&aperak_fv20261001::PROFILE);
    #[cfg(feature = "comdis")]
    _profiles.push(&comdis_fv20251001::PROFILE);
    #[cfg(feature = "comdis")]
    _profiles.push(&comdis_fv20261001::PROFILE);
    #[cfg(any(feature = "contrl-archive", feature = "archive"))]
    _profiles.push(&contrl_fv20251001::PROFILE);
    #[cfg(feature = "contrl")]
    _profiles.push(&contrl_fv20260101::PROFILE);
    #[cfg(feature = "iftsta")]
    _profiles.push(&iftsta_fv20251001::PROFILE);
    #[cfg(feature = "iftsta")]
    _profiles.push(&iftsta_fv20261001::PROFILE);
    #[cfg(any(feature = "insrpt-archive", feature = "archive"))]
    _profiles.push(&insrpt_fv20211001::PROFILE);
    #[cfg(feature = "insrpt")]
    _profiles.push(&insrpt_fv20260101::PROFILE);
    #[cfg(feature = "invoic")]
    _profiles.push(&invoic_fv20251001::PROFILE);
    #[cfg(feature = "invoic")]
    _profiles.push(&invoic_fv20260401::PROFILE);
    #[cfg(any(feature = "mscons-archive", feature = "archive"))]
    _profiles.push(&mscons_fv20240401::PROFILE);
    #[cfg(feature = "mscons")]
    _profiles.push(&mscons_fv20251001::PROFILE);
    #[cfg(feature = "mscons")]
    _profiles.push(&mscons_fv20261001::PROFILE);
    #[cfg(feature = "ordchg")]
    _profiles.push(&ordchg_fv20241001::PROFILE);
    #[cfg(feature = "ordchg")]
    _profiles.push(&ordchg_fv20260401::PROFILE);
    #[cfg(feature = "orders")]
    _profiles.push(&orders_fv20251001::PROFILE);
    #[cfg(feature = "orders")]
    _profiles.push(&orders_fv20260401::PROFILE);
    #[cfg(feature = "ordrsp")]
    _profiles.push(&ordrsp_fv20251001::PROFILE);
    #[cfg(feature = "ordrsp")]
    _profiles.push(&ordrsp_fv20260401::PROFILE);
    #[cfg(feature = "partin")]
    _profiles.push(&partin_fv20251001::PROFILE);
    #[cfg(feature = "partin")]
    _profiles.push(&partin_fv20260401::PROFILE);
    #[cfg(feature = "pricat")]
    _profiles.push(&pricat_fv20250401::PROFILE);
    #[cfg(feature = "pricat")]
    _profiles.push(&pricat_fv20260401::PROFILE);
    #[cfg(feature = "quotes")]
    _profiles.push(&quotes_fv20250401::PROFILE);
    #[cfg(feature = "quotes")]
    _profiles.push(&quotes_fv20260401::PROFILE);
    #[cfg(feature = "remadv")]
    _profiles.push(&remadv_fv20251001::PROFILE);
    #[cfg(feature = "remadv")]
    _profiles.push(&remadv_fv20260401::PROFILE);
    #[cfg(feature = "reqote")]
    _profiles.push(&reqote_fv20250401::PROFILE);
    #[cfg(feature = "reqote")]
    _profiles.push(&reqote_fv20260401::PROFILE);
    #[cfg(feature = "utilmd")]
    _profiles.push(&utilmd_fv20241001::PROFILE);
    #[cfg(feature = "utilmd")]
    _profiles.push(&utilmd_fv20241001_gas::PROFILE);
    #[cfg(feature = "utilmd")]
    _profiles.push(&utilmd_fv20250606::PROFILE);
    #[cfg(feature = "utilmd")]
    _profiles.push(&utilmd_fv20251001::PROFILE);
    #[cfg(feature = "utilmd")]
    _profiles.push(&utilmd_fv20251001_gas::PROFILE);
    #[cfg(feature = "utilmd")]
    _profiles.push(&utilmd_fv20261001::PROFILE);
    #[cfg(feature = "utilmd")]
    _profiles.push(&utilmd_fv20261001_gas::PROFILE);
    #[cfg(feature = "utilts")]
    _profiles.push(&utilts_fv20241001::PROFILE);
    #[cfg(feature = "utilts")]
    _profiles.push(&utilts_fv20260401::PROFILE);
}

/// Compile-time guard: every generated profile module must declare
/// `CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION`.
/// Regenerate with `cargo xtask codegen` if this fails.
#[allow(dead_code)]
pub(crate) const CURRENT_CODEGEN_SCHEMA_VERSION: u32 = 1;
#[cfg(feature = "aperak")]
const _: () = assert!(aperak_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "aperak")]
const _: () = assert!(aperak_fv20261001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "comdis")]
const _: () = assert!(comdis_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "comdis")]
const _: () = assert!(comdis_fv20261001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(any(feature = "contrl-archive", feature = "archive"))]
const _: () = assert!(contrl_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "contrl")]
const _: () = assert!(contrl_fv20260101::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "iftsta")]
const _: () = assert!(iftsta_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "iftsta")]
const _: () = assert!(iftsta_fv20261001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(any(feature = "insrpt-archive", feature = "archive"))]
const _: () = assert!(insrpt_fv20211001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "insrpt")]
const _: () = assert!(insrpt_fv20260101::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "invoic")]
const _: () = assert!(invoic_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "invoic")]
const _: () = assert!(invoic_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(any(feature = "mscons-archive", feature = "archive"))]
const _: () = assert!(mscons_fv20240401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "mscons")]
const _: () = assert!(mscons_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "mscons")]
const _: () = assert!(mscons_fv20261001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "ordchg")]
const _: () = assert!(ordchg_fv20241001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "ordchg")]
const _: () = assert!(ordchg_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "orders")]
const _: () = assert!(orders_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "orders")]
const _: () = assert!(orders_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "ordrsp")]
const _: () = assert!(ordrsp_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "ordrsp")]
const _: () = assert!(ordrsp_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "partin")]
const _: () = assert!(partin_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "partin")]
const _: () = assert!(partin_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "pricat")]
const _: () = assert!(pricat_fv20250401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "pricat")]
const _: () = assert!(pricat_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "quotes")]
const _: () = assert!(quotes_fv20250401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "quotes")]
const _: () = assert!(quotes_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "remadv")]
const _: () = assert!(remadv_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "remadv")]
const _: () = assert!(remadv_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "reqote")]
const _: () = assert!(reqote_fv20250401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "reqote")]
const _: () = assert!(reqote_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilmd")]
const _: () = assert!(utilmd_fv20241001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilmd")]
const _: () =
    assert!(utilmd_fv20241001_gas::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilmd")]
const _: () = assert!(utilmd_fv20250606::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilmd")]
const _: () = assert!(utilmd_fv20251001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilmd")]
const _: () =
    assert!(utilmd_fv20251001_gas::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilmd")]
const _: () = assert!(utilmd_fv20261001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilmd")]
const _: () =
    assert!(utilmd_fv20261001_gas::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilts")]
const _: () = assert!(utilts_fv20241001::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);
#[cfg(feature = "utilts")]
const _: () = assert!(utilts_fv20260401::CODEGEN_SCHEMA_VERSION == CURRENT_CODEGEN_SCHEMA_VERSION);

/// Well-known release identifiers for all registered profiles.
///
/// Use these instead of `Release::new("...")` to get a compile error
/// when a profile is removed or renamed after a BDEW format update.
///
/// # Example
/// ```rust,ignore
/// use edi_energy::releases;
/// use edi_energy::builders::MsconsBuilder;
/// let msg = MsconsBuilder::new(releases::mscons_fv20261001().clone())
///     .sender("9900000000002")
///     .receiver("9900000000003")
///     .build();
/// ```
pub mod releases {
    #[cfg(any(
        feature = "aperak",
        feature = "archive",
        feature = "comdis",
        feature = "contrl",
        feature = "contrl-archive",
        feature = "iftsta",
        feature = "insrpt",
        feature = "insrpt-archive",
        feature = "invoic",
        feature = "mscons",
        feature = "mscons-archive",
        feature = "ordchg",
        feature = "orders",
        feature = "ordrsp",
        feature = "partin",
        feature = "pricat",
        feature = "quotes",
        feature = "remadv",
        feature = "reqote",
        feature = "utilmd",
        feature = "utilts"
    ))]
    use crate::Release;
    #[cfg(any(
        feature = "aperak",
        feature = "archive",
        feature = "comdis",
        feature = "contrl",
        feature = "contrl-archive",
        feature = "iftsta",
        feature = "insrpt",
        feature = "insrpt-archive",
        feature = "invoic",
        feature = "mscons",
        feature = "mscons-archive",
        feature = "ordchg",
        feature = "orders",
        feature = "ordrsp",
        feature = "partin",
        feature = "pricat",
        feature = "quotes",
        feature = "remadv",
        feature = "reqote",
        feature = "utilmd",
        feature = "utilts"
    ))]
    use std::sync::LazyLock;

    /// Release `2.1i` — valid from profile directory `fv20251001`.
    #[cfg(feature = "aperak")]
    pub fn aperak_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.1i"));
        &R
    }

    /// Release `2.2` — valid from profile directory `fv20261001`.
    #[cfg(feature = "aperak")]
    pub fn aperak_fv20261001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.2"));
        &R
    }

    /// Release `1.0g` — valid from profile directory `fv20251001`.
    #[cfg(feature = "comdis")]
    pub fn comdis_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.0g"));
        &R
    }

    /// Release `1.0g` — valid from profile directory `fv20261001`.
    #[cfg(feature = "comdis")]
    pub fn comdis_fv20261001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.0g"));
        &R
    }

    /// Release `2.0b` — valid from profile directory `fv20251001`.
    /// This profile is archived. Enable `contrl-archive` or `archive` to use it.
    #[cfg(any(feature = "contrl-archive", feature = "archive"))]
    pub fn contrl_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.0b"));
        &R
    }

    /// Release `2.0b` — valid from profile directory `fv20260101`.
    #[cfg(feature = "contrl")]
    pub fn contrl_fv20260101() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.0b"));
        &R
    }

    /// Release `2.0g` — valid from profile directory `fv20251001`.
    #[cfg(feature = "iftsta")]
    pub fn iftsta_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.0g"));
        &R
    }

    /// Release `2.1` — valid from profile directory `fv20261001`.
    #[cfg(feature = "iftsta")]
    pub fn iftsta_fv20261001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.1"));
        &R
    }

    /// Release `1.1a` — valid from profile directory `fv20211001`.
    /// This profile is archived. Enable `insrpt-archive` or `archive` to use it.
    #[cfg(any(feature = "insrpt-archive", feature = "archive"))]
    pub fn insrpt_fv20211001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.1a"));
        &R
    }

    /// Release `1.1a` — valid from profile directory `fv20260101`.
    #[cfg(feature = "insrpt")]
    pub fn insrpt_fv20260101() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.1a"));
        &R
    }

    /// Release `2.8e` — valid from profile directory `fv20251001`.
    #[cfg(feature = "invoic")]
    pub fn invoic_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.8e"));
        &R
    }

    /// Release `2.8e` — valid from profile directory `fv20260401`.
    #[cfg(feature = "invoic")]
    pub fn invoic_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.8e"));
        &R
    }

    /// Release `2.4c` — valid from profile directory `fv20240401`.
    /// This profile is archived. Enable `mscons-archive` or `archive` to use it.
    #[cfg(any(feature = "mscons-archive", feature = "archive"))]
    pub fn mscons_fv20240401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.4c"));
        &R
    }

    /// Release `2.4c` — valid from profile directory `fv20251001`.
    #[cfg(feature = "mscons")]
    pub fn mscons_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.4c"));
        &R
    }

    /// Release `2.5` — valid from profile directory `fv20261001`.
    #[cfg(feature = "mscons")]
    pub fn mscons_fv20261001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.5"));
        &R
    }

    /// Release `1.1` — valid from profile directory `fv20241001`.
    #[cfg(feature = "ordchg")]
    pub fn ordchg_fv20241001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.1"));
        &R
    }

    /// Release `1.2` — valid from profile directory `fv20260401`.
    #[cfg(feature = "ordchg")]
    pub fn ordchg_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.2"));
        &R
    }

    /// Release `1.4b` — valid from profile directory `fv20251001`.
    #[cfg(feature = "orders")]
    pub fn orders_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.4b"));
        &R
    }

    /// Release `1.4c` — valid from profile directory `fv20260401`.
    #[cfg(feature = "orders")]
    pub fn orders_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.4c"));
        &R
    }

    /// Release `1.4b` — valid from profile directory `fv20251001`.
    #[cfg(feature = "ordrsp")]
    pub fn ordrsp_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.4b"));
        &R
    }

    /// Release `1.4c` — valid from profile directory `fv20260401`.
    #[cfg(feature = "ordrsp")]
    pub fn ordrsp_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.4c"));
        &R
    }

    /// Release `1.0f` — valid from profile directory `fv20251001`.
    #[cfg(feature = "partin")]
    pub fn partin_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.0f"));
        &R
    }

    /// Release `1.1` — valid from profile directory `fv20260401`.
    #[cfg(feature = "partin")]
    pub fn partin_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.1"));
        &R
    }

    /// Release `2.0e` — valid from profile directory `fv20250401`.
    #[cfg(feature = "pricat")]
    pub fn pricat_fv20250401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.0e"));
        &R
    }

    /// Release `2.1` — valid from profile directory `fv20260401`.
    #[cfg(feature = "pricat")]
    pub fn pricat_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.1"));
        &R
    }

    /// Release `1.3b` — valid from profile directory `fv20250401`.
    #[cfg(feature = "quotes")]
    pub fn quotes_fv20250401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.3b"));
        &R
    }

    /// Release `1.3c` — valid from profile directory `fv20260401`.
    #[cfg(feature = "quotes")]
    pub fn quotes_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.3c"));
        &R
    }

    /// Release `2.9e` — valid from profile directory `fv20251001`.
    #[cfg(feature = "remadv")]
    pub fn remadv_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.9e"));
        &R
    }

    /// Release `2.9f` — valid from profile directory `fv20260401`.
    #[cfg(feature = "remadv")]
    pub fn remadv_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("2.9f"));
        &R
    }

    /// Release `1.3c` — valid from profile directory `fv20250401`.
    #[cfg(feature = "reqote")]
    pub fn reqote_fv20250401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.3c"));
        &R
    }

    /// Release `1.3c` — valid from profile directory `fv20260401`.
    #[cfg(feature = "reqote")]
    pub fn reqote_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.3c"));
        &R
    }

    /// Release `S1.1a` — valid from profile directory `fv20241001`.
    #[cfg(feature = "utilmd")]
    pub fn utilmd_fv20241001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("S1.1a"));
        &R
    }

    /// Release `G1.0a` — valid from profile directory `fv20241001_gas`.
    #[cfg(feature = "utilmd")]
    pub fn utilmd_fv20241001_gas() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("G1.0a"));
        &R
    }

    /// Release `S1.2` — valid from profile directory `fv20250606`.
    #[cfg(feature = "utilmd")]
    pub fn utilmd_fv20250606() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("S1.2"));
        &R
    }

    /// Release `S2.1` — valid from profile directory `fv20251001`.
    #[cfg(feature = "utilmd")]
    pub fn utilmd_fv20251001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("S2.1"));
        &R
    }

    /// Release `G1.1` — valid from profile directory `fv20251001_gas`.
    #[cfg(feature = "utilmd")]
    pub fn utilmd_fv20251001_gas() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("G1.1"));
        &R
    }

    /// Release `S2.2` — valid from profile directory `fv20261001`.
    #[cfg(feature = "utilmd")]
    pub fn utilmd_fv20261001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("S2.2"));
        &R
    }

    /// Release `G1.2` — valid from profile directory `fv20261001_gas`.
    #[cfg(feature = "utilmd")]
    pub fn utilmd_fv20261001_gas() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("G1.2"));
        &R
    }

    /// Release `1.1e` — valid from profile directory `fv20241001`.
    #[cfg(feature = "utilts")]
    pub fn utilts_fv20241001() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.1e"));
        &R
    }

    /// Release `1.1e` — valid from profile directory `fv20260401`.
    #[cfg(feature = "utilts")]
    pub fn utilts_fv20260401() -> &'static Release {
        static R: LazyLock<Release> = LazyLock::new(|| Release::new("1.1e"));
        &R
    }
}
