//! Role separation, asserted per role build.
//!
//! processd's Cargo features decide which Marktrolle's decision paths are
//! compiled and registered. This file is what keeps that a property rather than
//! a comment: an `nb-only` build must not answer a message only a Lieferant or
//! a Messstellenbetreiber ever receives.
//!
//! **What this is not.** It is not a regulatory control. §7 EnWG requires a
//! separate legal entity for a VNB at or above 100 000 connected customers
//! (§7 Abs. 2 is the de-minimis threshold), §7a operational and §6a
//! informational separation; none of them is discharged by a build flag and no
//! BNetzA procedure inspects a binary. Unbundling is decided above this
//! repository. What these assertions buy is narrower and still worth having: a
//! deployment cannot answer as a role it was not built for, so a
//! misconfiguration fails closed instead of putting an unauthorised
//! Willenserklärung on the market.
//!
//! Run per feature set:
//!
//! ```bash
//! cargo test -p processd --no-default-features --features nb-only  --test role_separation
//! cargo test -p processd --no-default-features --features lf-only  --test role_separation
//! cargo test -p processd --no-default-features --features msb-only --test role_separation
//! ```

use processd::handler::answerable_pids;

/// PIDs that belong to a supplier's own answer obligation.
///
/// Strom 55007/55010/55016 and their GeLi Gas twins 44007/44010/44016. All six
/// arrive at the *old* supplier, who is the only party holding the
/// Lieferverhältnis the answer is decided from.
#[allow(dead_code)] // unused in an `integrated` build, where no exclusion applies
const LF_PIDS: &[u32] = &[44_007, 44_010, 44_016, 55_007, 55_010, 55_016];
/// PIDs the Netzbetreiber answers.
///
/// 55016 „Kündigung" is deliberately absent: the Anwendungsübersicht 4.0 has it
/// going LFN → LFA, so it is a supplier obligation the NB never receives.
#[allow(dead_code)]
const NB_PIDS: &[u32] = &[55_001, 55_077, 55_004, 44_001, 44_004, 55_042, 55_051];
/// PIDs the Messstellenbetreiber answers (MSB-Wechsel side).
#[allow(dead_code)]
const MSB_WECHSEL_PIDS: &[u32] = &[55_039, 55_168];
/// PIDs the Messstellenbetreiber answers toward an Energieserviceanbieter
/// (WiM Teil 2 Kap. 4).
///
/// Serving an ESA is a mandatory Zusatzleistung (§34 Abs. 2 S. 2 Nr. 10 MsbG),
/// so an MSB build owes all four — but no NB or LF ever receives one, which is
/// what makes them a role-separation marker.
#[allow(dead_code)]
const MSB_ESA_PIDS: &[u32] = &[35_003, 17_007, 17_008, 39_002];

#[allow(dead_code)]
fn assert_disjoint(pids: &[u32], forbidden: &[u32], role: &str, other: &str) {
    let leaked: Vec<u32> = pids
        .iter()
        .copied()
        .filter(|p| forbidden.contains(p))
        .collect();
    assert!(
        leaked.is_empty(),
        "a {role} build answers {other} PIDs {leaked:?} — the role features exist so a \
         deployment cannot put a Willenserklärung on the market for a Marktrolle it was \
         not built for"
    );
}

#[cfg(all(
    any(feature = "role-nb-strom", feature = "role-nb-gas"),
    not(any(feature = "role-lf-strom", feature = "role-lf-gas")),
    not(feature = "role-msb")
))]
#[test]
fn an_nb_only_build_answers_no_lf_or_msb_process() {
    let pids = answerable_pids();
    assert_disjoint(&pids, LF_PIDS, "nb-only", "LF");
    assert_disjoint(&pids, MSB_WECHSEL_PIDS, "nb-only", "MSB");
    assert_disjoint(&pids, MSB_ESA_PIDS, "nb-only", "MSB/ESA");
    assert!(
        pids.contains(&55_001) && pids.contains(&55_042),
        "an nb-only build must still answer the NB's own processes, got {pids:?}"
    );
    // The Abmeldung (55004 Strom / 44004 Gas) and the Anmeldung erz. MaLo
    // (55077) are NB obligations too, not just the verbrauchende Anmeldung.
    for pid in [55_004, 55_077, 44_004] {
        assert!(
            pids.contains(&pid),
            "the NB owes an answer to {pid}, got {pids:?}"
        );
    }
    // 55016 „Kündigung" is LFN → LFA — a supplier process. Answering it from an
    // NB binary is the § 6a EnWG leak these features exist to prevent.
    assert!(
        !pids.contains(&55_016),
        "55016 is a supplier obligation (LFN → LFA), not the NB's"
    );
    // The Ende MSB (55051) is MSBA → NB, so the NB owes its answer within 7 WT.
    assert!(
        pids.contains(&55_051),
        "the NB owes an answer to 55051 Ende MSB, got {pids:?}"
    );
}

#[cfg(all(
    any(feature = "role-lf-strom", feature = "role-lf-gas"),
    not(any(feature = "role-nb-strom", feature = "role-nb-gas")),
    not(feature = "role-msb")
))]
#[test]
fn an_lf_only_build_answers_no_nb_or_msb_process() {
    let pids = answerable_pids();
    assert_disjoint(&pids, NB_PIDS, "lf-only", "NB");
    assert_disjoint(&pids, MSB_WECHSEL_PIDS, "lf-only", "MSB");
    assert_disjoint(&pids, MSB_ESA_PIDS, "lf-only", "MSB/ESA");

    // Sparte-scoped: the expected set is the enabled Spartes' PIDs, so a
    // `role-lf-strom` build that answered a GeLi Gas Abmeldung would fail here.
    let mut expected: Vec<u32> = Vec::new();
    if cfg!(feature = "role-lf-gas") {
        // 44013 is the Anmeldung E/G — the one Anmeldung a supplier is asked to
        // check, because it is the one the GNB *assigns* to it (§ 36 / § 38
        // EnWG). Its Zustimmung takes a second opt-in, but the walk and its
        // Frist belong to every LF build.
        expected.extend_from_slice(&[44_007, 44_010, 44_013, 44_016]);
    }
    if cfg!(feature = "role-lf-strom") {
        // 55607 is Strom-only: assigning a supplier to an erzeugende
        // Marktlokation is a Bilanzkreis mechanic GeLi Gas has no counterpart
        // for.
        expected.extend_from_slice(&[55_007, 55_010, 55_013, 55_016, 55_607]);
    }
    assert_eq!(
        pids, expected,
        "an lf-only build answers exactly the supplier's inbound processes"
    );
}

#[cfg(all(
    feature = "role-msb",
    not(any(feature = "role-nb-strom", feature = "role-nb-gas")),
    not(any(feature = "role-lf-strom", feature = "role-lf-gas"))
))]
#[test]
fn an_msb_only_build_answers_no_nb_or_lf_process() {
    let pids = answerable_pids();
    assert_disjoint(&pids, NB_PIDS, "msb-only", "NB");
    assert_disjoint(&pids, LF_PIDS, "msb-only", "LF");
    // 55039 Kündigung MSB is MSBN → MSBA: the MSB is the only role that can
    // receive it, so it must compile into an msb-only binary.
    assert!(
        pids.contains(&55_039),
        "the MSB owes an answer to 55039 Kündigung MSB, got {pids:?}"
    );
    // Serving an ESA is mandatory under §34 Abs. 2 S. 2 Nr. 10 MsbG, so an
    // msb-only build must carry all four Kapitel-4 obligations — an MSB that
    // silently answered none of them would breach the Zusatzleistung.
    for pid in MSB_ESA_PIDS {
        assert!(
            pids.contains(pid),
            "the MSB owes an answer to {pid} (WiM Teil 2 Kap. 4), got {pids:?}"
        );
    }
}

/// Whatever the build, a PID is never claimed by two roles at once.
#[test]
fn the_answerable_set_has_no_duplicates() {
    let pids = answerable_pids();
    let mut sorted = pids.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        pids, sorted,
        "answerable_pids() must be sorted and deduplicated"
    );
}

/// 55003–55006 are the NB's *answers* to an Anmeldung, and 55008/55009/55011/
/// 55012 the LF's answers. `makod` never emits `process.initiated` for an
/// answer PID, so listening for one is a module that never fires.
#[test]
fn no_answer_pid_is_ever_answerable() {
    // 55004 is NOT here: it is the LF's *Abmeldung*, an inbound trigger the NB
    // answers with 55005/55006. Listing it as an answer PID is what kept the
    // NB's Lieferende obligation from ever being wired.
    const ANSWER_PIDS: &[u32] = &[
        55_002, 55_003, 55_005, 55_006, 55_008, 55_009, 55_011, 55_012, 55_017, 55_018, 55_040,
        55_041, 55_043, 55_044, 55_052, 55_053, 55_078, 55_080, 55_169, 55_170,
    ];
    let pids = answerable_pids();
    let leaked: Vec<u32> = pids
        .iter()
        .copied()
        .filter(|p| ANSWER_PIDS.contains(p))
        .collect();
    assert!(
        leaked.is_empty(),
        "these are answer PIDs, not inbound triggers: {leaked:?} — `makod` only emits \
         `process.initiated` for the message that spawned the process"
    );
}
