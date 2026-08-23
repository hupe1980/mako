// ── PID ownership across the WiM Teil 1 ORDERS and ORDRSP families ───────────

/// Each ORDERS PID belongs to exactly one process, per the *Anwendungsübersicht
/// der Prüfidentifikatoren* 4.0.
///
/// Six ORDERS PIDs sit next to each other in the WiM Teil 1 band and only two
/// belong here. The others differ in direction, Frist and Entscheidungsbaum:
///
/// | PID   | Anwendungsfall                             | von → an      | Owner |
/// |-------|--------------------------------------------|---------------|-------|
/// | 17001 | Bestellung Geräteübernahmeangebot          | MSBN → MSBA   | `geraeteubernahme` |
/// | 17009 | Anzeige Gerätewechselabsicht               | MSBN → MSBA   | `geraeteubernahme` |
/// | 17002 | Weiterverpflichtung                        | **NB → MSBA** | `weiterverpflichtung` |
/// | 17005 | Bestellung Rechnungsabwicklung MSB über LF | **LF → MSB**  | `rechnungsabwicklung` |
/// | 17006 | Beendigung Rechnungsabwicklung MSB über LF | LF ↔ MSB      | `rechnungsabwicklung` |
/// | 17011 | Bestellung Angebot Änderung Technik        | NB / LF → MSB | `technik_aenderung` |
///
/// The **Anforderung** eines Geräteübernahmeangebots (Kap. 3.2.2 Nr. 1) is not
/// an ORDERS at all: it is REQOTE 35001, answered by QUOTES 15001.
#[test]
fn geraeteubernahme_owns_only_its_own_orders_pids() {
    use mako_wim::geraeteubernahme::GERAETEUBERNAHME_PIDS;

    assert_eq!(
        GERAETEUBERNAHME_PIDS,
        &[17_001, 17_009],
        "the Geräteübernahme ORDERS set is fixed by WiM Strom Teil 1 Kap. 3.1/3.2"
    );

    for foreign in [17_002_u32, 17_005, 17_006, 17_011] {
        assert!(
            !GERAETEUBERNAHME_PIDS.contains(&foreign),
            "PID {foreign} belongs to another process and must not be claimed here"
        );
    }
}

/// Every WiM ORDERS PID is claimed by exactly one workflow — an overlap would
/// make routing order decide which workflow sees the message.
#[test]
fn the_orders_pid_sets_of_the_wim_workflows_are_disjoint() {
    use mako_wim::geraeteubernahme::GERAETEUBERNAHME_PIDS;
    use mako_wim::rechnungsabwicklung::RECHNUNGSABWICKLUNG_ORDERS_PIDS as RA_PIDS;
    use mako_wim::technik_aenderung::ORDERS_PIDS as TECHNIK_PIDS;
    use mako_wim::weiterverpflichtung::AUFTRAG_PID as WV_PID;

    assert!(
        TECHNIK_PIDS.contains(&17_011),
        "17011 Bestellung Angebot Änderung Technik belongs to technik_aenderung"
    );
    assert_eq!(WV_PID, 17_002);

    let sets: [(&str, &[u32]); 4] = [
        ("wim-geraeteubernahme", GERAETEUBERNAHME_PIDS),
        ("wim-technik-aenderung", TECHNIK_PIDS),
        ("wim-rechnungsabwicklung", RA_PIDS),
        ("wim-weiterverpflichtung", &[WV_PID]),
    ];
    for (i, (name_a, a)) in sets.iter().enumerate() {
        for (name_b, b) in &sets[i + 1..] {
            for pid in *a {
                assert!(
                    !b.contains(pid),
                    "PID {pid} is claimed by both {name_a} and {name_b}"
                );
            }
        }
    }
}

/// The ORDRSP answers split the same way: 19003/19004 answer the
/// Weiterverpflichtung (`E_0203`), not the Technikänderung (`E_0249`/`E_0250`,
/// which answers on 19005/19006).
///
/// Source: ORDRSP AHB 1.1b §§ 4.9.2, 4.10, 4.11, 4.12.
#[test]
fn the_ordrsp_answers_follow_their_own_orders() {
    use mako_wim::geraeteubernahme::{ABLEHNUNG_PID, BESTAETIGUNG_PID, GERAETEWECHSELABSICHT_PIDS};
    use mako_wim::technik_aenderung::ORDRSP_PIDS as TECHNIK_ORDRSP;
    use mako_wim::weiterverpflichtung::ANTWORT_PIDS as WV_ANTWORT;

    assert_eq!(WV_ANTWORT, (19_003, 19_004));
    assert_eq!(TECHNIK_ORDRSP, &[19_005, 19_006]);
    assert_eq!(BESTAETIGUNG_PID.as_u32(), 19_001);
    assert_eq!(ABLEHNUNG_PID.as_u32(), 19_002);
    assert_eq!(GERAETEWECHSELABSICHT_PIDS, (19_015, 19_016));

    for pid in [WV_ANTWORT.0, WV_ANTWORT.1] {
        assert!(
            !TECHNIK_ORDRSP.contains(&pid),
            "PID {pid} answers the Weiterverpflichtung, not a Technikänderung"
        );
    }
    // 19007 „Ablehnung Anforderung von Werten" answers a Werteanforderung and
    // is not part of the Technikänderung either.
    assert!(!TECHNIK_ORDRSP.contains(&19_007));
}
