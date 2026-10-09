//! The answer table must agree with the AHB names of the PIDs it names.
//!
//! [`answer_pids`] maps an Anfrage to its `(Bestätigung, Ablehnung)` pair. The
//! pair is a hand-maintained table because BDEW's `+1/+2` numbering does not
//! hold everywhere — and a hand-maintained table is exactly what drifts.
//!
//! The AHB profiles carry the authoritative German name for every PID
//! ("Bestätigung Anmeldung verb. MaLo", "Ablehnung Abmeldung", …), so a swapped
//! or shifted pair is detectable: the positive answer's name must begin with
//! *Bestätigung* and the negative one's with *Ablehnung*.
//!
//! The numbering shifted at FV2025-10-01, so prose naming 55002 "Anfrage
//! Lieferende" or 55003 "Bestätigung Lieferbeginn" is off by one against the
//! shipped profiles while the code around it is right — the direction that is
//! hard to spot by reading, and the one this test decides mechanically.
//!
//! Names are read from the profile JSON rather than the runtime registry
//! because codegen does not carry the PID name into the generated profile.

use std::collections::BTreeMap;
use std::path::PathBuf;

use edi_energy::{ablehnung_pid, answer_pids, bestaetigung_pid};

fn profiles_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/profiles"))
}

/// `PID -> every name any shipped profile gives it`.
///
/// A PID can carry more than one name across releases: BDEW renumbered the
/// Kündigung family at FV2025-10-01 (55017 went from *Kündigung* to
/// *Bestätigung Kündigung*), so both spellings are shipped. Any one of them
/// matching is enough — the table only has to be right for some release.
fn pid_names() -> BTreeMap<u32, Vec<String>> {
    let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let root = profiles_root();
    for ty in std::fs::read_dir(&root).expect("profiles dir").flatten() {
        if !ty.path().is_dir() {
            continue;
        }
        for release in std::fs::read_dir(ty.path()).into_iter().flatten().flatten() {
            let Ok(raw) = std::fs::read_to_string(release.path().join("ahb.json")) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
                continue;
            };
            for e in v["anwendungsfaelle"].as_array().into_iter().flatten() {
                if let (Some(code), Some(name)) = (e["pid"].as_u64(), e["name"].as_str()) {
                    out.entry(code as u32)
                        .or_default()
                        .push(name.trim().to_owned());
                }
            }
        }
    }
    out
}

/// Every Anfrage this table knows. Kept explicit so a new family added to
/// `answer_pids` without a line here is a visible omission rather than a
/// silently unchecked entry.
const ANFRAGE_PIDS: &[u32] = &[
    55001, 55004, 55016, 55077, // GPKE Strom
    44001, 44004, 44007, 44010, 44013, 44016, // GeLi Gas
];

fn any_starts_with(names: &[String], prefix: &str) -> bool {
    names.iter().any(|n| n.starts_with(prefix))
}

#[test]
fn answer_table_matches_the_ahb_names() {
    let names = pid_names();
    assert!(
        names.len() > 100,
        "only {} PID names parsed — the scan is broken, not the table",
        names.len()
    );

    let mut wrong = Vec::new();

    for &anfrage in ANFRAGE_PIDS {
        let Some((ok, nok)) = answer_pids(anfrage) else {
            wrong.push(format!(
                "{anfrage}: listed as an Anfrage here but answer_pids returned None"
            ));
            continue;
        };

        // The Anfrage itself must not be named like an answer.
        if let Some(n) = names.get(&anfrage)
            && (any_starts_with(n, "Bestätigung") || any_starts_with(n, "Ablehnung"))
        {
            wrong.push(format!(
                "{anfrage} is used as an Anfrage but the AHB names it {n:?}"
            ));
        }

        // A PID absent from every shipped profile is an AHB *coverage* gap
        // (tracked separately); this test only judges the entries that exist.
        if let Some(n) = names.get(&ok)
            && !any_starts_with(n, "Bestätigung")
        {
            wrong.push(format!(
                "{anfrage} → Bestätigung {ok}, but the AHB names {ok} {n:?}"
            ));
        }

        if let Some(n) = names.get(&nok)
            && !any_starts_with(n, "Ablehnung")
        {
            wrong.push(format!(
                "{anfrage} → Ablehnung {nok}, but the AHB names {nok} {n:?}"
            ));
        }
    }

    assert!(
        wrong.is_empty(),
        "the answer_pids table disagrees with the shipped AHB profiles:\n  {}",
        wrong.join("\n  ")
    );
}

/// The asymmetric families the pair-returning API cannot express.
#[test]
fn asymmetric_families_agree_with_the_ahb() {
    let names = pid_names();

    // GeLi Gas 44020 is confirmable but never rejectable.
    let b = bestaetigung_pid(44020).expect("44020 has a Bestätigung");
    assert_eq!(b, 44021);
    if let Some(n) = names.get(&b) {
        // The AHB names 44021 „Antwort auf Änderungsmeldung zur
        // Bestandsliste": one answer that carries both outcomes.
        assert!(
            any_starts_with(n, "Bestätigung") || any_starts_with(n, "Antwort"),
            "44020 → Bestätigung {b}, but the AHB names it {n:?}"
        );
    }
    assert_eq!(
        ablehnung_pid(44020),
        None,
        "44020 has no Ablehnung in the AHB — inventing one would put an \
         undefined PID on the wire"
    );
    assert_eq!(answer_pids(44020), None, "an incomplete pair must be None");

    // 44019 has neither answer.
    assert_eq!(bestaetigung_pid(44019), None);
    assert_eq!(ablehnung_pid(44019), None);
}

/// An answer PID must never itself be treated as an Anfrage.
#[test]
fn answers_are_not_requests() {
    for &anfrage in ANFRAGE_PIDS {
        let (ok, nok) = answer_pids(anfrage).expect("checked above");
        assert_eq!(
            answer_pids(ok),
            None,
            "{ok} is the Bestätigung of {anfrage}; treating it as an Anfrage \
             would answer an answer"
        );
        assert_eq!(
            answer_pids(nok),
            None,
            "{nok} is the Ablehnung of {anfrage}"
        );
    }
}
