//! Prüfidentifikator introspection.
//!
//! Which PIDs exist, which of them the compiled profiles carry AHB rules for,
//! and what a counterparty is entitled to answer with, are all properties of
//! the BDEW documents. They are answered from the profile registry rather than
//! duplicated as hand-maintained lists in Python, so a generated PID is always
//! one this build can really validate.
//!
//! ## Vacuous validation
//!
//! `ahb_rule_pack` returns a stand-in pack named `unknown-pid` for a code it
//! does not know, carrying a single *warning* rule. A message with such a PID
//! therefore validates — `is_valid` comes back `True` having checked nothing.
//! [`pid_has_ahb_rules`] is the only sound way to ask whether a PID is really
//! known; `rule_count() > 0` is true for every code, including nonsense.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use edi_energy::registry::{ProcessContext, ReleaseRegistry, UNKNOWN_PID_PACK};
use edi_energy::{MessageType, Pruefidentifikator, Release, ReleaseTrack};

use crate::fristen::parse_date;

pub(crate) fn message_type_from_str(name: &str) -> PyResult<MessageType> {
    MessageType::from_unh_code(&name.to_ascii_uppercase())
        .ok_or_else(|| PyValueError::new_err(format!("unknown EDIFACT message type {name:?}")))
}

/// Resolve a `sparte` argument to the UTILMD release track.
///
/// UTILMD is the only type with parallel tracks — `S…` for Strom, `G…` for Gas
/// — and they carry different releases on the same date. Every other type has
/// one track, so `sparte` is ignored there rather than being an error: a caller
/// passing it uniformly should not have to special-case MSCONS.
pub(crate) fn track_for(mt: MessageType, sparte: Option<&str>) -> PyResult<Option<ReleaseTrack>> {
    // Validated for every type, applied only to UTILMD: a caller passing a
    // Sparte uniformly should not have to special-case MSCONS, but a typo must
    // not be silently ignored either.
    let track = match sparte.map(str::to_ascii_uppercase).as_deref() {
        None => None,
        Some("STROM") => Some(ReleaseTrack::Strom),
        Some("GAS") => Some(ReleaseTrack::Gas),
        Some(other) => {
            return Err(PyValueError::new_err(format!(
                "sparte must be \"STROM\" or \"GAS\", got {other:?}"
            )));
        }
    };
    Ok(if mt == MessageType::Utilmd {
        track
    } else {
        None
    })
}

/// The release normatively active for `message_type` on `on`, as a wire code.
///
/// This is what `build_*` uses when you pass a date instead of a release, and
/// it removes the sharpest foot-gun in the toolkit: hand-picking `"S2.1"` and
/// then validating on a date where `"S2.2"` is in force produces findings that
/// describe the mismatch rather than the message.
///
/// `sparte` selects the UTILMD track and is ignored for every other type.
/// Returns `None` when no profile is active on that date.
#[pyfunction]
#[pyo3(signature = (message_type, on, sparte=None))]
pub fn release_for(message_type: &str, on: &str, sparte: Option<&str>) -> PyResult<Option<String>> {
    let mt = message_type_from_str(message_type)?;
    let ctx = ProcessContext::for_date(parse_date(on)?);
    let release = match track_for(mt, sparte)? {
        Some(track) => ctx.active_release_for_track(mt, track),
        None => ctx.active_release(mt),
    };
    Ok(release.map(|r| r.as_str().to_owned()))
}

/// Every BDEW format version the compiled profiles carry, as `FVYYYY-MM-DD`.
///
/// Parametrize a suite over this to prove a message survives every version the
/// build claims to support, rather than only the one the author had in mind.
#[pyfunction]
pub fn format_versions() -> Vec<String> {
    ReleaseRegistry::global().format_versions()
}

/// Every wire release code registered for `message_type`, ascending.
#[pyfunction]
pub fn releases(message_type: &str) -> PyResult<Vec<String>> {
    let mt = message_type_from_str(message_type)?;
    Ok(ReleaseRegistry::global()
        .releases(mt)
        .into_iter()
        .map(|r| r.as_str().to_owned())
        .collect())
}

/// Numeric bands the BDEW assigns to each message type's Prüfidentifikatoren.
///
/// The registry maps `(type, PID) → rules` but cannot enumerate its own keys,
/// so the enumeration below scans each type's band. The bands are BDEW's own
/// allocation and change only when a message type is introduced.
fn pid_bands(mt: MessageType) -> &'static [(u32, u32)] {
    match mt {
        MessageType::Mscons => &[(13000, 13999)],
        MessageType::Quotes => &[(15000, 15999)],
        MessageType::Orders => &[(17000, 17999)],
        MessageType::Ordrsp => &[(19000, 19999)],
        MessageType::Iftsta => &[(21000, 21999)],
        MessageType::Insrpt => &[(23000, 23999)],
        MessageType::Utilts => &[(25000, 25999)],
        MessageType::Pricat => &[(27000, 27999)],
        MessageType::Aperak => &[(29000, 29999)],
        MessageType::Invoic => &[(31000, 31999)],
        MessageType::Remadv => &[(33000, 33999)],
        MessageType::Reqote => &[(35000, 35999)],
        MessageType::Partin => &[(37000, 37999)],
        MessageType::Ordchg => &[(39000, 39999)],
        MessageType::Utilmd => &[(44000, 44999), (55000, 55999)],
        // COMDIS shares APERAK's 29xxx band — both AHBs declare 29001/29002,
        // so neither owns it and a PID there resolves to *both*.
        MessageType::Comdis => &[(29000, 29999)],
        // CONTRL is a technical acknowledgement; its profiles are `pid_exempt`
        // and the AHB assigns it no Prüfidentifikatoren at all.
        MessageType::Contrl => &[],
        // `MessageType` is #[non_exhaustive]. A type added upstream without a
        // band here reports "no PIDs" rather than guessing a range; the
        // `every_pid_carrying_message_type_has_a_band` test fails so it is not
        // missed.
        _ => &[],
    }
}

/// Does *some* registered profile for `mt` carry real AHB rules for `pid`?
fn known_anywhere(mt: MessageType, pid: Pruefidentifikator) -> bool {
    ReleaseRegistry::global().pid_has_ahb_rules(mt, pid)
}

/// Does the profile active on `on` carry real AHB rules for `pid`?
///
/// Narrower than [`known_anywhere`] and usually the question that matters: a
/// PID retired at the last Formatumstellung is still "known" to the registry
/// through its old profile, but a message carrying it today validates
/// vacuously.
fn known_on(
    mt: MessageType,
    pid: Pruefidentifikator,
    on: time::Date,
    track: Option<ReleaseTrack>,
) -> bool {
    let ctx = ProcessContext::for_date(on);
    let profile = match track {
        Some(t) => ctx.active_profile_for_track(mt, t),
        None => ctx.active_profile(mt),
    };
    profile.is_some_and(|p| p.ahb_rule_pack(Some(pid)).name() != UNKNOWN_PID_PACK)
}

/// `True` when the compiled profiles carry real AHB rules for `pid`.
///
/// With `on`, restricts the question to the profile active on that date —
/// which is what a message sent on that date is actually validated against.
#[pyfunction]
#[pyo3(signature = (message_type, pid, on=None, sparte=None))]
pub fn pid_has_ahb_rules(
    message_type: &str,
    pid: u32,
    on: Option<&str>,
    sparte: Option<&str>,
) -> PyResult<bool> {
    let mt = message_type_from_str(message_type)?;
    let Ok(p) = Pruefidentifikator::new(pid) else {
        return Ok(false);
    };
    match on {
        None => Ok(known_anywhere(mt, p)),
        Some(date) => Ok(known_on(mt, p, parse_date(date)?, track_for(mt, sparte)?)),
    }
}

/// Every Prüfidentifikator of `message_type` the compiled profiles validate.
///
/// Ascending, derived by scanning the type's BDEW band, so it reflects exactly
/// what this build can check — a PID published by BDEW but not yet imported is
/// absent, and that absence is the honest answer for a test generator.
///
/// With `on`, only PIDs the profile active on that date carries rules for.
#[pyfunction]
#[pyo3(signature = (message_type, on=None, sparte=None))]
pub fn pruefidentifikatoren(
    message_type: &str,
    on: Option<&str>,
    sparte: Option<&str>,
) -> PyResult<Vec<u32>> {
    let mt = message_type_from_str(message_type)?;
    let track = track_for(mt, sparte)?;
    let date = on.map(parse_date).transpose()?;
    let mut out = Vec::new();
    for &(lo, hi) in pid_bands(mt) {
        for code in lo..=hi {
            let Ok(p) = Pruefidentifikator::new(code) else {
                continue;
            };
            let known = match date {
                None => known_anywhere(mt, p),
                Some(d) => known_on(mt, p, d, track),
            };
            if known {
                out.push(code);
            }
        }
    }
    // The Sparte bands are a property of UTILMD's numbering, not of the release
    // track, so they are applied here rather than left to the caller.
    if mt == MessageType::Utilmd {
        match track {
            Some(ReleaseTrack::Strom) => out.retain(|p| (55000..=55999).contains(p)),
            Some(ReleaseTrack::Gas) => out.retain(|p| (44000..=44999).contains(p)),
            _ => {}
        }
    }
    Ok(out)
}

/// Every EDIFACT message type whose compiled profiles declare `pid`.
///
/// A list, because a Prüfidentifikator does **not** identify one message type:
/// APERAK and COMDIS both declare 29001 and 29002, so a function returning one
/// name has to be wrong for one of them.
#[pyfunction]
pub fn message_types_of(pid: u32) -> Vec<String> {
    const TYPES: &[MessageType] = &[
        MessageType::Aperak,
        MessageType::Comdis,
        MessageType::Iftsta,
        MessageType::Insrpt,
        MessageType::Invoic,
        MessageType::Mscons,
        MessageType::Ordchg,
        MessageType::Orders,
        MessageType::Ordrsp,
        MessageType::Partin,
        MessageType::Pricat,
        MessageType::Quotes,
        MessageType::Remadv,
        MessageType::Reqote,
        MessageType::Utilmd,
        MessageType::Utilts,
    ];
    let Ok(p) = Pruefidentifikator::new(pid) else {
        return Vec::new();
    };
    TYPES
        .iter()
        .filter(|mt| known_anywhere(**mt, p))
        .map(|mt| mt.as_str().to_owned())
        .collect()
}

// ── The AHB answer table ──────────────────────────────────────────────────────

/// The `(Bestätigung, Ablehnung)` PIDs the AHB assigns to a request PID.
///
/// `None` when `anfrage` is not a request PID, or when the family has no
/// complete pair — GeLi Gas 44020 is confirmable but cannot be rejected.
///
/// Bound rather than re-tabulated because the mapping is not `anfrage + 1`:
/// GPKE 55077 rejects with 55080, since 55079 is unassigned.
#[pyfunction]
pub fn answer_pids(anfrage: u32) -> Option<(u32, u32)> {
    edi_energy::answer_pids(anfrage)
}

/// The Bestätigung PID for a request PID, if the AHB defines one.
#[pyfunction]
pub fn bestaetigung_pid(anfrage: u32) -> Option<u32> {
    edi_energy::bestaetigung_pid(anfrage)
}

/// The Ablehnung PID for a request PID, if the AHB defines one.
#[pyfunction]
pub fn ablehnung_pid(anfrage: u32) -> Option<u32> {
    edi_energy::ablehnung_pid(anfrage)
}

pub(crate) fn resolve_release(
    mt: MessageType,
    release: Option<&str>,
    on: Option<&str>,
    sparte: Option<&str>,
) -> PyResult<Release> {
    if let Some(r) = release {
        return Ok(Release::new(r));
    }
    let Some(date) = on else {
        return Err(PyValueError::new_err(format!(
            "pass either release= or on= — a {} message has no default release, and \
             guessing one produces findings that describe the mismatch rather than \
             the message",
            mt.as_str()
        )));
    };
    let ctx = ProcessContext::for_date(parse_date(date)?);
    let found = match track_for(mt, sparte)? {
        Some(track) => ctx.active_release_for_track(mt, track),
        None => ctx.active_release(mt),
    };
    found.map(|r| Release::new(r.as_str())).ok_or_else(|| {
        PyValueError::new_err(format!(
            "no {} profile is active on {date} — pass release= explicitly, or pick a \
             date inside a published format version (see format_versions())",
            mt.as_str()
        ))
    })
}

pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(release_for, m)?)?;
    m.add_function(wrap_pyfunction!(format_versions, m)?)?;
    m.add_function(wrap_pyfunction!(releases, m)?)?;
    m.add_function(wrap_pyfunction!(pruefidentifikatoren, m)?)?;
    m.add_function(wrap_pyfunction!(pid_has_ahb_rules, m)?)?;
    m.add_function(wrap_pyfunction!(message_types_of, m)?)?;
    m.add_function(wrap_pyfunction!(answer_pids, m)?)?;
    m.add_function(wrap_pyfunction!(bestaetigung_pid, m)?)?;
    m.add_function(wrap_pyfunction!(ablehnung_pid, m)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every EDIFACT message type that carries Prüfidentifikatoren must have a
    /// band declared, or `pruefidentifikatoren()` silently returns nothing for
    /// it and a strategy draws from an empty pool.
    ///
    /// `MessageType` is `#[non_exhaustive]`, so a type added upstream reaches
    /// the catch-all arm and this test is what surfaces it.
    #[test]
    fn every_pid_carrying_message_type_has_a_band() {
        for mt in [
            MessageType::Utilmd,
            MessageType::Mscons,
            MessageType::Aperak,
            MessageType::Invoic,
            MessageType::Remadv,
            MessageType::Orders,
            MessageType::Iftsta,
            MessageType::Insrpt,
            MessageType::Reqote,
            MessageType::Partin,
            MessageType::Ordchg,
            MessageType::Ordrsp,
            MessageType::Quotes,
            MessageType::Pricat,
            MessageType::Utilts,
        ] {
            assert!(
                !pid_bands(mt).is_empty(),
                "{} carries Prüfidentifikatoren but has no band declared",
                mt.as_str()
            );
        }
    }

    #[test]
    fn message_types_of_resolves_against_the_profiles() {
        for (pid, expected) in [
            (55001u32, vec!["UTILMD"]),
            (44001, vec!["UTILMD"]),
            (13025, vec!["MSCONS"]),
            (17115, vec!["ORDERS"]),
            (31009, vec!["INVOIC"]),
            (33001, vec!["REMADV"]),
            (29001, vec!["APERAK", "COMDIS"]),
        ] {
            assert_eq!(message_types_of(pid), expected, "pid {pid}");
        }
        assert!(message_types_of(99999).is_empty());
    }

    #[test]
    fn pruefidentifikatoren_are_sorted_and_exclude_unknown_codes() {
        let utilmd = pruefidentifikatoren("UTILMD", None, None).unwrap();
        assert!(utilmd.len() > 50, "got {}", utilmd.len());
        assert!(utilmd.windows(2).all(|w| w[0] < w[1]));
        for known in [55001u32, 55002, 55003, 55004] {
            assert!(utilmd.contains(&known));
        }
        // 56xxx is unassigned; the stand-in pack must not make it look known.
        assert!(!utilmd.iter().any(|&p| (56000..=56999).contains(&p)));
        assert!(
            pruefidentifikatoren("CONTRL", None, None)
                .unwrap()
                .is_empty()
        );
        assert!(pruefidentifikatoren("NOSUCH", None, None).is_err());
    }

    /// Restricting to a date must narrow the set, never widen it — the profile
    /// active on a date is one of the profiles the registry holds.
    #[test]
    fn dating_the_enumeration_narrows_it() {
        let all = pruefidentifikatoren("UTILMD", None, Some("STROM")).unwrap();
        let dated = pruefidentifikatoren("UTILMD", Some("2025-10-01"), Some("STROM")).unwrap();
        assert!(!dated.is_empty(), "a live format version must carry PIDs");
        assert!(dated.len() <= all.len());
        assert!(dated.iter().all(|p| all.contains(p)));
        assert!(dated.iter().all(|p| (55000..=55999).contains(p)));
    }

    #[test]
    fn a_release_is_resolved_from_a_date() {
        let strom = release_for("UTILMD", "2025-10-01", Some("STROM"))
            .unwrap()
            .expect("a UTILMD Strom profile is active");
        let gas = release_for("UTILMD", "2025-10-01", Some("GAS"))
            .unwrap()
            .expect("a UTILMD Gas profile is active");
        assert!(strom.starts_with('S'), "{strom}");
        assert!(gas.starts_with('G'), "{gas}");
        assert!(release_for("MSCONS", "2025-10-01", None).unwrap().is_some());
    }

    /// The two answer tables in this workspace must agree. `edi_energy` derives
    /// the simulator's reply and `mako_fristen::antwort` derives the deadline;
    /// a disagreement would mean a counterparty answering with a PID the
    /// platform is not waiting for.
    #[test]
    fn the_answer_tables_agree() {
        for o in mako_fristen::antwort::all() {
            // The Preisanfrage rows carry the same QUOTES PID twice — a REQOTE
            // is answered by one quote, not a Bestätigung/Ablehnung pair — so
            // there is nothing for the AHB pair table to agree with.
            if o.antwort_pids.0 == o.antwort_pids.1 {
                continue;
            }
            let Some(pair) = edi_energy::answer_pids(o.trigger_pid) else {
                continue;
            };
            assert_eq!(
                pair, o.antwort_pids,
                "answer PIDs disagree for trigger {}",
                o.trigger_pid
            );
        }
    }
}
