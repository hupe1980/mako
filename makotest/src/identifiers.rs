//! BDEW identifier construction and validation.
//!
//! Every function here delegates to `rubo4e::identifiers`, the same code the
//! platform validates with. None of the arithmetic is reimplemented: a harness
//! carrying its own copy of the BDEW check-digit procedures
//! ("Identifikatoren in der Marktkommunikation" v1.2 §8.1/§8.2) or of the
//! ENTSO-E check character would generate values the platform refuses.
//!
//! ## Why generation, not just validation
//!
//! A random 11-digit string is a valid Marktlokations-ID with probability 1/10,
//! and a random 16-character EIC essentially never. A test that invents one
//! exercises the rejection path while claiming to test the happy path — the
//! failure mode is a green suite that proves nothing. Every identifier family
//! the platform accepts therefore has a `*_from_base` / `*_from_prefix`
//! constructor here.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use rubo4e::identifiers::{
    BilanzierungsgebietId, BilanzkreisId, CrId, EicCode, MaloId, MarktpartnerId, MeloId, NebeId,
    NeloId, PaketId, SgId, SrId, TrId,
};

fn value_err(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}

// ── Marktlokation ─────────────────────────────────────────────────────────────

/// `True` when `value` is a check-digit-valid 11-digit Marktlokations-ID.
#[pyfunction]
pub fn malo_is_valid(value: &str) -> bool {
    MaloId::new(value).is_ok()
}

/// The BDEW §8.1 check digit for a 10-digit MaLo base.
///
/// Raises `ValueError` when `base` is not exactly 10 digits, or when its first
/// digit is `0` — position 1 is the Codevergabestelle and `0` is unissued.
#[pyfunction]
pub fn malo_check_digit(base: &str) -> PyResult<u8> {
    MaloId::check_digit(base).map_err(value_err)
}

/// Complete a 10-digit base into a valid 11-digit Marktlokations-ID.
#[pyfunction]
pub fn malo_from_base(base: &str) -> PyResult<String> {
    MaloId::from_base(base)
        .map(|id| id.as_ref().to_owned())
        .map_err(value_err)
}

// ── Messlokation ──────────────────────────────────────────────────────────────

/// `True` when `value` is a well-formed 33-character Messlokations-ID.
///
/// A MeLo carries no check digit — the constraint is length, the two-letter
/// country prefix and the `[A-Z0-9]` body — so there is no `melo_from_base`.
#[pyfunction]
pub fn melo_is_valid(value: &str) -> bool {
    MeloId::new(value).is_ok()
}

// ── Marktpartner ──────────────────────────────────────────────────────────────

/// `True` when `value` is 13 decimal digits — the structural test alone.
///
/// This is what the platform enforces at its boundary. It deliberately does
/// **not** check the check digit: §2.3 defines two different procedures (BDEW
/// §8.1 and GS1/EAN-13) and the leading digits do not reliably say which
/// applies, so rejecting on a guess would drop identifiers that are in
/// production use. Use [`mp_id_check_digit_schemes`] to ask which one it
/// satisfies.
#[pyfunction]
pub fn mp_id_is_valid(value: &str) -> bool {
    MarktpartnerId::new(value).is_ok()
}

/// Every check-digit procedure `value` satisfies — `"bdew"`, `"gln"`, or both.
///
/// A **list**, because the two arithmetics agree on roughly one base in ten and
/// a code can genuinely be valid under both. An empty list means it satisfies
/// neither and every conformant counterparty would refuse it, which is the
/// answer an invented 13-digit fixture gets.
#[pyfunction]
pub fn mp_id_check_digit_schemes(value: &str) -> Vec<String> {
    let Ok(id) = MarktpartnerId::new(value) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if id.has_valid_bdew_check_digit() {
        out.push("bdew".to_owned());
    }
    if id.has_valid_gln_check_digit() {
        out.push("gln".to_owned());
    }
    out
}

/// Complete a 12-digit base into a 13-digit Marktpartner-ID.
///
/// `scheme` selects the check-digit procedure — `"bdew"` for a BDEW- or
/// DVGW-Codenummer (§8.1, the `99…`/`98…` ranges) and `"gln"` for a GS1 Global
/// Location Number (EAN-13). The two disagree on almost every base, and the
/// prefix does not decide it: pick the one matching the counterparty you are
/// simulating.
///
/// The `"gln"` digit is found by asking `rubo4e` which of the ten candidates it
/// accepts, so this cannot drift from the validator the platform uses.
#[pyfunction]
#[pyo3(signature = (base, scheme="bdew"))]
pub fn mp_id_from_base(base: &str, scheme: &str) -> PyResult<String> {
    match scheme {
        "bdew" | "dvgw" => MarktpartnerId::from_base(base)
            .map(|id| id.as_ref().to_owned())
            .map_err(value_err),
        "gln" => {
            if base.len() != 12 || !base.bytes().all(|b| b.is_ascii_digit()) {
                return Err(PyValueError::new_err(format!(
                    "a GLN base is exactly 12 digits, got {base:?}"
                )));
            }
            (0..10u8)
                .map(|d| format!("{base}{d}"))
                .find(|candidate| {
                    MarktpartnerId::new(candidate).is_ok_and(|id| id.has_valid_gln_check_digit())
                })
                .ok_or_else(|| {
                    PyValueError::new_err(format!("no GLN check digit completes {base:?}"))
                })
        }
        other => Err(PyValueError::new_err(format!(
            "unknown check-digit scheme {other:?} — expected \"bdew\" or \"gln\""
        ))),
    }
}

/// The issuing authority `value`'s prefix implies: `"BDEW"`, `"DVGW"` or `"GS1 GLN"`.
#[pyfunction]
pub fn mp_id_authority(value: &str) -> PyResult<String> {
    Ok(MarktpartnerId::new(value)
        .map_err(value_err)?
        .authority()
        .to_string())
}

/// The **UNB DE0007** qualifier for `value` — `"500"`, `"502"` or `"14"`.
///
/// This is what the interchange envelope writes after the party ID, and it is
/// derived from the ID rather than chosen: a mismatch is a Syntaxfehler the
/// counterparty rejects with a CONTRL.
///
/// There is deliberately no `nad_agency` counterpart. The NAD DE3055 code is
/// not a pure function of the ID in EDI@Energy practice — a BDEW-Codenummer is
/// itself GS1-issued, and the AHBs write `293` for it rather than the GS1 `9` —
/// so a helper that derived one would encode a guess about the AHB. The
/// builders' own default stands, and a caller who knows better overrides it.
#[pyfunction]
pub fn mp_id_unb_qualifier(value: &str) -> PyResult<&'static str> {
    Ok(MarktpartnerId::new(value)
        .map_err(value_err)?
        .authority()
        .unb_agency_code())
}

// ── EIC-based identifiers ─────────────────────────────────────────────────────

/// Complete a 15-character EIC prefix with its ENTSO-E check character.
///
/// Raises `ValueError` when the prefix is not 15 ASCII characters or when its
/// check character would be `-`, which ENTSO-E prohibits — such a prefix has no
/// valid completion and must be redrawn rather than patched.
#[pyfunction]
pub fn eic_from_prefix(prefix: &str) -> PyResult<String> {
    EicCode::new_from_prefix(prefix)
        .map(|c| c.as_ref().to_owned())
        .map_err(value_err)
}

/// `True` when `value` is a 16-character EIC with a correct check character.
#[pyfunction]
pub fn eic_is_valid(value: &str) -> bool {
    EicCode::new(value).is_ok()
}

/// The ENTSO-E object type of `value` — position 3, e.g. `"X"` for a party.
#[pyfunction]
pub fn eic_type_char(value: &str) -> PyResult<String> {
    Ok(EicCode::new(value)
        .map_err(value_err)?
        .type_char()
        .to_string())
}

/// Complete a 15-character prefix into a **Bilanzkreis**-ID (`11X…`).
///
/// A Bilanzkreis is held by a Bilanzkreisverantwortlicher, so ENTSO-E types it
/// as a **Party** — object type `X` at position 3.
#[pyfunction]
pub fn bilanzkreis_from_prefix(prefix: &str) -> PyResult<String> {
    BilanzkreisId::from_prefix(prefix)
        .map(|c| c.as_ref().to_owned())
        .map_err(value_err)
}

/// `True` when `value` is a valid Bilanzkreis-ID (EIC object type `X`).
#[pyfunction]
pub fn bilanzkreis_is_valid(value: &str) -> bool {
    BilanzkreisId::new(value).is_ok()
}

/// Complete a 15-character prefix into a **Bilanzierungsgebiet**-ID (`11Y…`).
///
/// A Bilanzierungsgebiet is a grid area, so ENTSO-E types it as an **Area** —
/// object type `Y`. Nothing but that character separates it from a Bilanzkreis:
/// both are 16 characters, both carry a valid check character, and MSCONS SG6
/// carries both as free text under different `LOC` qualifiers. A series filed
/// against the wrong one is a misfiling the BIKO cannot tell from a correct
/// submission, which is why the two have separate constructors here.
#[pyfunction]
pub fn bilanzierungsgebiet_from_prefix(prefix: &str) -> PyResult<String> {
    BilanzierungsgebietId::from_prefix(prefix)
        .map(|c| c.as_ref().to_owned())
        .map_err(value_err)
}

/// `True` when `value` is a valid Bilanzierungsgebiet-ID (EIC object type `Y`).
#[pyfunction]
pub fn bilanzierungsgebiet_is_valid(value: &str) -> bool {
    BilanzierungsgebietId::new(value).is_ok()
}

// ── §8.2 ASCII identifiers (NeLo, NeBe, Redispatch resources, Paket) ──────────

/// The seven BDEW §8.2 identifier families, by the name `resource_id_*` takes.
///
/// The Codetyp is the first character (two for the Paket-ID) and is fixed by
/// the BDEW document, so it is part of the base rather than something a caller
/// chooses.
const ASCII_KINDS: &[(&str, &str)] = &[
    ("nelo", "E"),
    ("nebe", "F"),
    ("cr", "A"),
    ("sg", "B"),
    ("sr", "C"),
    ("tr", "D"),
    ("paket", "P9"),
];

fn ascii_kind_prefix(kind: &str) -> PyResult<&'static str> {
    ASCII_KINDS
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, prefix)| *prefix)
        .ok_or_else(|| {
            let known: Vec<&str> = ASCII_KINDS.iter().map(|(n, _)| *n).collect();
            PyValueError::new_err(format!(
                "unknown resource-ID kind {kind:?} — expected one of {known:?}"
            ))
        })
}

/// Complete a 10-character base into an 11-character §8.2 identifier.
///
/// `kind` is one of `nelo`, `nebe`, `cr`, `sg`, `sr`, `tr`, `paket`. The base
/// must already start with that family's Codetyp (`E`, `F`, `A`, `B`, `C`, `D`,
/// `P9`) and be `[A-Z0-9]` throughout; the check digit is appended.
///
/// These are the identifiers a UTILMD transaction carries for a Netzlokation or
/// a Redispatch resource, so a test that builds one needs to be able to
/// generate one.
#[pyfunction]
pub fn resource_id_from_base(kind: &str, base: &str) -> PyResult<String> {
    let prefix = ascii_kind_prefix(kind)?;
    let out = match prefix {
        "E" => NeloId::from_base(base).map(|i| i.as_ref().to_owned()),
        "F" => NebeId::from_base(base).map(|i| i.as_ref().to_owned()),
        "A" => CrId::from_base(base).map(|i| i.as_ref().to_owned()),
        "B" => SgId::from_base(base).map(|i| i.as_ref().to_owned()),
        "C" => SrId::from_base(base).map(|i| i.as_ref().to_owned()),
        "D" => TrId::from_base(base).map(|i| i.as_ref().to_owned()),
        _ => PaketId::from_base(base).map(|i| i.as_ref().to_owned()),
    };
    out.map_err(value_err)
}

/// `True` when `value` is a valid identifier of family `kind`.
#[pyfunction]
pub fn resource_id_is_valid(kind: &str, value: &str) -> PyResult<bool> {
    let prefix = ascii_kind_prefix(kind)?;
    Ok(match prefix {
        "E" => NeloId::new(value).is_ok(),
        "F" => NebeId::new(value).is_ok(),
        "A" => CrId::new(value).is_ok(),
        "B" => SgId::new(value).is_ok(),
        "C" => SrId::new(value).is_ok(),
        "D" => TrId::new(value).is_ok(),
        _ => PaketId::new(value).is_ok(),
    })
}

/// The Codetyp prefix each `resource_id_*` kind requires, as `{kind: prefix}`.
///
/// Exposed so a generator can build a base without hardcoding the letters.
#[pyfunction]
pub fn resource_id_kinds() -> Vec<(String, String)> {
    ASCII_KINDS
        .iter()
        .map(|(k, p)| ((*k).to_owned(), (*p).to_owned()))
        .collect()
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(malo_is_valid, m)?)?;
    m.add_function(wrap_pyfunction!(malo_check_digit, m)?)?;
    m.add_function(wrap_pyfunction!(malo_from_base, m)?)?;
    m.add_function(wrap_pyfunction!(melo_is_valid, m)?)?;

    m.add_function(wrap_pyfunction!(mp_id_is_valid, m)?)?;
    m.add_function(wrap_pyfunction!(mp_id_check_digit_schemes, m)?)?;
    m.add_function(wrap_pyfunction!(mp_id_from_base, m)?)?;
    m.add_function(wrap_pyfunction!(mp_id_authority, m)?)?;
    m.add_function(wrap_pyfunction!(mp_id_unb_qualifier, m)?)?;

    m.add_function(wrap_pyfunction!(eic_from_prefix, m)?)?;
    m.add_function(wrap_pyfunction!(eic_is_valid, m)?)?;
    m.add_function(wrap_pyfunction!(eic_type_char, m)?)?;
    m.add_function(wrap_pyfunction!(bilanzkreis_from_prefix, m)?)?;
    m.add_function(wrap_pyfunction!(bilanzkreis_is_valid, m)?)?;
    m.add_function(wrap_pyfunction!(bilanzierungsgebiet_from_prefix, m)?)?;
    m.add_function(wrap_pyfunction!(bilanzierungsgebiet_is_valid, m)?)?;

    m.add_function(wrap_pyfunction!(resource_id_from_base, m)?)?;
    m.add_function(wrap_pyfunction!(resource_id_is_valid, m)?)?;
    m.add_function(wrap_pyfunction!(resource_id_kinds, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The BDEW §8.1 worked example, so the binding is pinned to the document
    /// rather than to whatever `rubo4e` happens to compute.
    #[test]
    fn malo_matches_the_bdew_reference_vector() {
        assert_eq!(malo_from_base("4137355924").unwrap(), "41373559241");
        assert!(malo_is_valid("41373559241"));
        assert!(!malo_is_valid("41373559242"));
    }

    /// Both procedures must be reachable, and they must disagree — if they
    /// produced the same digit the `scheme` argument would be decoration.
    #[test]
    fn the_two_mp_id_schemes_are_different_arithmetic() {
        let bdew = mp_id_from_base("990035700000", "bdew").unwrap();
        let gln = mp_id_from_base("990035700000", "gln").unwrap();
        assert_eq!(bdew, "9900357000003");
        assert_ne!(bdew, gln, "§8.1 and EAN-13 must not be the same digit");
        assert!(mp_id_check_digit_schemes(&bdew).contains(&"bdew".to_owned()));
        assert!(mp_id_check_digit_schemes(&gln).contains(&"gln".to_owned()));
        assert_eq!(mp_id_unb_qualifier(&bdew).unwrap(), "500");
    }

    /// A code that satisfies neither procedure must report neither. This is the
    /// class of value a hand-written fixture produces.
    #[test]
    fn an_invented_mp_id_satisfies_no_scheme() {
        assert!(
            mp_id_is_valid("9900357000004"),
            "13 digits, structurally ok"
        );
        assert!(mp_id_check_digit_schemes("9900357000004").is_empty());
    }

    /// The object type is the only thing separating the two EIC families, so
    /// each constructor must refuse the other's code.
    #[test]
    fn bilanzkreis_and_bilanzierungsgebiet_do_not_accept_each_other() {
        let bk = bilanzkreis_from_prefix("11XSWKIEL------").unwrap();
        let bg = bilanzierungsgebiet_from_prefix("11YSWKIEL------").unwrap();
        assert!(bilanzkreis_is_valid(&bk) && !bilanzierungsgebiet_is_valid(&bk));
        assert!(bilanzierungsgebiet_is_valid(&bg) && !bilanzkreis_is_valid(&bg));
        assert_eq!(eic_type_char(&bk).unwrap(), "X");
        assert_eq!(eic_type_char(&bg).unwrap(), "Y");
    }

    #[test]
    fn every_ascii_family_round_trips_through_its_own_validator() {
        for (kind, prefix) in ASCII_KINDS {
            let base = format!("{prefix}{}", "0".repeat(10 - prefix.len()));
            let id = resource_id_from_base(kind, &base).unwrap();
            assert_eq!(id.len(), 11, "{kind}");
            assert!(resource_id_is_valid(kind, &id).unwrap(), "{kind}: {id}");
        }
        assert!(resource_id_from_base("nosuch", "E000000001").is_err());
    }
}
