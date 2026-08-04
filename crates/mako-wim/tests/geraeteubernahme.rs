// ── PID ownership across WiM Teil 1 ORDERS ───────────────────────────────────

/// Each ORDERS PID belongs to exactly one process, per the *Anwendungsübersicht
/// der Prüfidentifikatoren* 4.0.
///
/// The Geräteübernahme set drifted here before: the makod dispatcher and adapter
/// also matched 17005 and 17011, and described 17005 as "NB → MSB". Both are
/// different processes with different senders:
///
/// | PID   | Anwendungsfall                             | von → an    | Owner |
/// |-------|--------------------------------------------|-------------|-------|
/// | 17001 | Bestellung Geräteübernahmeangebot          | MSBN → MSBA | `geraeteubernahme` |
/// | 17002 | Weiterverpflichtung                        | NB → MSBA   | `geraeteubernahme` |
/// | 17009 | Anzeige Gerätewechselabsicht               | MSBN → MSBA | `geraeteubernahme` |
/// | 17005 | Bestellung Rechnungsabwicklung MSB über LF | **LF → MSB**| *(not implemented)* |
/// | 17006 | Beendigung Rechnungsabwicklung MSB über LF | LF ↔ MSB    | *(not implemented)* |
/// | 17011 | Bestellung Angebot Änderung Technik        | NB / LF → MSB | `technik_aenderung` |
#[test]
fn geraeteubernahme_owns_only_its_own_orders_pids() {
    use mako_wim::geraeteubernahme::GERAETEUBERNAHME_PIDS;

    assert_eq!(
        GERAETEUBERNAHME_PIDS,
        &[17_001, 17_002, 17_009],
        "the Geräteübernahme ORDERS set is fixed by WiM Strom Teil 1"
    );

    for foreign in [17_005_u32, 17_006, 17_011] {
        assert!(
            !GERAETEUBERNAHME_PIDS.contains(&foreign),
            "PID {foreign} belongs to another process and must not be claimed here"
        );
    }
}

/// 17011 (Änderung Technik) is owned by its own workflow, and the two ORDERS
/// sets must not overlap — an overlap would make routing order decide which
/// workflow sees the message.
#[test]
fn the_orders_pid_sets_of_the_wim_workflows_are_disjoint() {
    use mako_wim::geraeteubernahme::GERAETEUBERNAHME_PIDS;
    use mako_wim::technik_aenderung::ORDERS_PIDS as TECHNIK_PIDS;

    assert!(
        TECHNIK_PIDS.contains(&17_011),
        "17011 Bestellung Angebot Änderung Technik belongs to technik_aenderung"
    );
    for pid in GERAETEUBERNAHME_PIDS {
        assert!(
            !TECHNIK_PIDS.contains(pid),
            "PID {pid} is claimed by both wim-geraeteubernahme and wim-technik-aenderung"
        );
    }
}
