//! Guard: every routed Prüfidentifikator must carry AHB rules.
//!
//! For a PID it does not know, `Profile::ahb_rule_pack` returns a stand-in pack
//! named `unknown-pid` whose only rule raises a **warning** —
//! "Pruefidentifikator is not registered for this release — AHB rules were not
//! applied". `report.is_valid()` stays `true`, so a PID that mako routes but
//! that never made it into the profile passes the AHB layer unchecked. The MIG
//! layer still applies, and the warning is the only trace.
//!
//! That is not hypothetical. ORDERS 17008/17116/17117 were lost because only the
//! first column of each multi-PID AHB table survived import, and the UTILMD
//! profiles are missing a large block of published PIDs for the same reason.
//! `validate-profiles` catches a PID *lost between releases*; it cannot catch one
//! that was never imported. This test closes that hole by cross-checking the
//! routers — the set of PIDs mako actually accepts — against the profiles.
//!
//! Uses only committed code and profiles: no PDFs, no network, CI-safe.

use std::collections::BTreeMap;

use edi_energy::{MessageType, Platform, Pruefidentifikator};
use mako_engine::builder::EngineModule;
use mako_engine::marktrolle::DeploymentRoles;
use mako_engine::pid_router::PidRouter;

/// PIDs deliberately routed without AHB rules, with the reason.
///
/// Keep this empty where possible. An entry is a promise that the PID's rules
/// are genuinely not applicable — not a place to park an import gap.
const RULELESS_BY_DESIGN: &[(u32, &str)] = &[
    // CONTRL is a technical acknowledgement: the AHB defines no
    // Prüfidentifikatoren for it at all (its profiles are `pid_exempt`).
];

/// Routed PIDs whose AHB rules were never imported — an acknowledged backlog.
///
/// Cross-checked against the BDEW **PID overview 4.0 (01.04.2026)**: the UTILMD
/// profiles carry 110 of 189 published Strom PIDs and 50 of 88 Gas PIDs, and
/// ORDERS 35 of 44. These are the subset that a workflow actually routes, so
/// they are the ones with live consequences: the message is accepted and its AHB
/// rules are not applied.
///
/// This list must only ever shrink. Clear entries by re-importing the profile
/// (`cargo xtask extract-pdf`, then complete each draft with the MIG segments as
/// `O`). A PID *not* in this list that lacks rules is a new regression.
const KNOWN_PROFILE_GAPS: &[u32] = &[
    // GeLi Gas Bestandsliste / Änderungsmeldung (UTILMD AHB Gas § 5.8).
    // 44007–44016 — the four processes an LF must answer — were curated from
    // UTILMD AHB Gas 1.2 §§ 5.3/5.4/5.6/5.7 and so are absent from this list.
    44019, 44020, 44021, // GeLi Gas Stammdatenänderung band
    44137, 44138, 44139, 44140, 44142, 44143, 44145, 44146, 44147, 44148, 44149, 44150, 44151,
    44152, 44156, 44157, 44162, 44163, 44164, 44165, 44166, 44167, 44180, 44181, 44182,
    // GPKE: NB-initiated Lieferende, erzeugende MaLo, MSB-Abrechnungsdaten,
    // Stammdatenänderung. 55230/55232 (Blindarbeits-Abrechnungsdaten der NeLo,
    // LF → NB) and 55557/55559 (MSB-Abrechnungsdaten der MaLo, MSB → NB) are
    // GPKE Teil 4 Stammdaten-Prozessschritte 1/2 like the rest of the band.
    // 55156/55220/55673 are the GPKE Teil 2 § 3.1 Rückmeldung/Bestellung
    // Abrechnungsdaten answered by IFTSTA 21047.
    // 55007/55607 (with their answers) were curated from UTILMD AHB Strom 2.2
    // §§ 8.10 and 8.15 and so are absent from this list.
    55077, 55078, 55080, 55156, 55220, 55230, 55232, 55557, 55559, 55673,
    // MaBiS-ZP lifecycle (`mabis-zp-lifecycle`) — Aktivierung/Deaktivierung of
    // the MaBiS-Zählpunkt, the Zuordnungsermächtigung and the AAÜZ/LF-AASZR
    // series, with their Antwort and Weiterleitung codes.
    //
    // The UTILMD AHB Strom PDF carries all of these, and `extract-pdf`
    // reproduces them — but the drafts mark strictly more segments `M` than the
    // AHB requires (the group-flattening margin), so promoting them unreviewed
    // would reject valid messages rather than merely fail to check them. They
    // are routed because the workflow is real and the alternative is dropping
    // the message entirely; validation stays vacuous until the entries are
    // curated.
    55062, 55063, 55064, 55071, 55072, 55197, 55198, 55199, 55200, 55203, 55204, 55205, 55206,
    55207, 55208, 55209, 55210, 55211, 55212, 55213, 55214,
    // MaBiS Anforderungen (`mabis-anforderung`) — ORDERS 17201–17208. Same
    // curation gate as the UTILMD band above: the ORDERS AHB carries them, the
    // extracted drafts are stricter than the AHB, so they stay uncurated.
    17201, 17202, 17203, 17204, 17205, 17206, 17207, 17208,
    // MaBiS Listenabgleich (`mabis-listenabgleich`) — the three list/correction
    // pairs. Same curation gate as the bands above.
    55195, 55196, 55201, 55202, 55223, 55224,
];

/// Message type a PID belongs to, from its leading digits.
///
/// 29xxx is declared by both APERAK and COMDIS. Either resolves the same way
/// here — both profiles carry identical rules for 29001/29002 — so the coarse
/// mapping is sufficient for a has-rules check.
fn message_type_of(pid: u32) -> Option<MessageType> {
    Some(match pid / 1000 {
        13 => MessageType::Mscons,
        15 => MessageType::Quotes,
        17 => MessageType::Orders,
        19 => MessageType::Ordrsp,
        21 => MessageType::Iftsta,
        23 => MessageType::Insrpt,
        25 => MessageType::Utilts,
        27 => MessageType::Pricat,
        29 => MessageType::Aperak,
        31 => MessageType::Invoic,
        33 => MessageType::Remadv,
        35 => MessageType::Reqote,
        37 => MessageType::Partin,
        39 => MessageType::Ordchg,
        44 | 55 => MessageType::Utilmd,
        _ => return None,
    })
}

#[tokio::test]
async fn every_routed_pid_has_ahb_rules() {
    let modules: Vec<Box<dyn EngineModule>> = vec![
        Box::new(mako_gpke::GpkeModule),
        Box::new(mako_wim::WimModule),
        Box::new(mako_geli_gas::GeliGasModule),
        Box::new(mako_gabi_gas::GaBiGasModule),
        Box::new(mako_mabis::MabisModule),
        Box::new(mako_redispatch::RedispatchModule),
    ];
    let roles = DeploymentRoles::all();
    let platform = Platform::with_all_profiles();

    // (pid -> workflow) across every module, each in its own router so the
    // deliberate cross-module PID overlaps do not collide.
    let mut routed: BTreeMap<u32, String> = BTreeMap::new();
    for m in &modules {
        let mut router = PidRouter::new();
        m.register_pids_with_roles(&mut router, &roles);
        for pid in router.registered_pids() {
            if let Some(wf) = router.route(pid) {
                routed.entry(pid).or_insert_with(|| wf.to_owned());
            }
        }
        for (pid, _sparte, wf) in router.registered_commodity_entries() {
            routed.entry(pid).or_insert_with(|| wf.to_owned());
        }
    }
    assert!(!routed.is_empty(), "expected registered PIDs");

    let exempt: BTreeMap<u32, &str> = RULELESS_BY_DESIGN.iter().copied().collect();
    let mut gaps: Vec<String> = Vec::new();
    let mut closed: Vec<u32> = Vec::new();

    for (pid, workflow) in &routed {
        if exempt.contains_key(pid) {
            continue;
        }
        let Some(mt) = message_type_of(*pid) else {
            continue;
        };
        let Ok(p) = Pruefidentifikator::new(*pid) else {
            continue;
        };

        // A PID has rules when *some* shipped profile of its message type
        // defines them. Release selection is a separate concern
        // (`validate-profiles` enforces continuity); the question here is only
        // whether the PID was ever imported at all.
        //
        // The discriminator is the pack *name*, not its rule count: the
        // stand-in for an unknown PID carries exactly one (warning) rule, so
        // counting rules would accept every gap.
        let has_rules = platform
            .registry()
            .profiles_for(mt)
            .any(|prof| prof.ahb_rule_pack(Some(p)).name() != "unknown-pid");

        match (has_rules, KNOWN_PROFILE_GAPS.contains(pid)) {
            (false, false) => gaps.push(format!("  {pid} (routed by `{workflow}`, {mt:?})")),
            (true, true) => closed.push(*pid),
            _ => {}
        }
    }

    assert!(
        closed.is_empty(),
        "these PIDs now have AHB rules — remove them from KNOWN_PROFILE_GAPS so the \
         list keeps shrinking: {closed:?}"
    );

    assert!(
        gaps.is_empty(),
        "these PIDs are routed but have no AHB rules in any shipped profile, so \
         inbound messages carrying them are validated against an empty rule pack \
         and silently pass:\n{}\n\nRe-import the profile from the BDEW AHB (see \
         `cargo xtask extract-pdf`). If a PID is genuinely rule-free, add it to \
         RULELESS_BY_DESIGN with the reason.",
        gaps.join("\n")
    );
}
