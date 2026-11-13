//! The ORDERS columns a workflow queues render and pass the pre-send gate.
//!
//! Each test drives the workflow that owns the column, takes the ORDERS its
//! own `handle` queues, renders it as the outbox worker does and holds the
//! result to `is_sendable()` — ORDERS AHB 1.1b Kap. 5.1 (17201–17205), 5.2
//! (17206–17208), 5.4 (17210) and 5.5 (17211).

use edi_energy::EdiEnergyMessage as _;
use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::{OutboxMessage, PendingOutbox};
use mako_engine::types::{MarktpartnerCode, MessageRef};
use mako_engine::workflow::Workflow;
use makod::config::PartyConfig;
use makod::edifact_renderer::render_to_wire_bytes;
use makod::party_registry::MpIdRegistry;

const LF: &str = "9900000000027";
const NB: &str = "9900357000004";
const BIKO: &str = "9900000000010";
const MABIS_ZP: &str = "DE0001112223334445556667778889990";

/// Render `entry` as the outbox worker does and hold it to the pre-send gate.
fn sendable(entry: &PendingOutbox) -> String {
    let (text, report) = rendered(entry);
    assert!(
        report.is_sendable(),
        "the pre-send gate refuses it: {:?}\n{text}",
        report.sending_refusals().collect::<Vec<_>>()
    );
    text
}

/// Render `entry` as the outbox worker does; the wire text and the gate's
/// report.
fn rendered(entry: &PendingOutbox) -> (String, edi_energy::EdiEnergyReport) {
    let msg = OutboxMessage::new(
        StreamId::new("process/orders-columns"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        entry.message_type.clone(),
        entry.recipient.clone(),
        entry.payload.clone(),
    );
    let registry = MpIdRegistry::from_config(&[PartyConfig {
        mp_id: LF.to_owned(),
        roles: vec!["LF".to_owned()],
        primary: true,
        agency: None,
        as4: None,
    }])
    .expect("registry");
    let wire = render_to_wire_bytes(&msg, &registry).expect("renders");
    let text = String::from_utf8_lossy(&wire.bytes).into_owned();
    let report = edi_energy::parse(&wire.bytes)
        .expect("parses")
        .validate_on_date(mako_fristen::heute())
        .expect("validation runs");
    (text, report)
}

/// **Each MaBiS Anforderung renders its column.** ORDERS AHB 1.1b Kap. 5.1
/// (S. 116–119): `BGM` `Z19` / `Z05` / `E40` by PID, `IMD` `Z01` / `Z02` /
/// `Z03`, `DTM+273` beside ohne Abo, `SG30 CCI` the Profilgruppe on 17201,
/// `SG34 RFF+AUU` and `SG38 LOC+172` on 17202 / 17204 / 17205, and on 17203
/// `SG38 LOC+237` the Bilanzkreis and, ohne Abo, `SG2 LOC+107`.
///
/// The 17202 / 17203 places conditioned on `[1]` / `[33]` / `[34]` („Wenn
/// IMD+Z03 vorhanden") pass too: the gate reads the AHB's shorthand `IMD+Z03`
/// in DE 7081, where the ORDERS MIG 1.4c (S. 20) puts the Abonnement.
#[test]
fn every_mabis_anforderung_renders_a_sendable_column() {
    use mako_mabis::anforderung::{
        AbonnementVorgang as V, AnforderungAngaben, AnforderungCommand, AnforderungKind as K,
        MabisAnforderungWorkflow as W,
    };
    let queue = |kind: K, vorgang: V, receiver: &str, angaben: AnforderungAngaben| {
        let out = W::handle(
            &Default::default(),
            AnforderungCommand::SendAnforderung {
                kind,
                vorgang,
                receiver: MarktpartnerCode::new(receiver),
                sender: MarktpartnerCode::new(LF),
                message_ref: MessageRef::new("ANF-4711"),
                angaben,
            },
        )
        .expect("accepted");
        out.outbox[0].clone()
    };
    let send = |kind: K, vorgang: V, receiver: &str, angaben: AnforderungAngaben| {
        sendable(&queue(kind, vorgang, receiver, angaben))
    };
    let unchecked = |kind: K, vorgang: V, receiver: &str, angaben: AnforderungAngaben| {
        sendable(&queue(kind, vorgang, receiver, angaben))
    };
    let monat = || Some("202610".to_owned());
    let version = || Some("20261105120000+00".to_owned());

    let t = send(
        K::NormierteProfile,
        V::Bestellung,
        NB,
        AnforderungAngaben {
            profilgruppe: Some("Z02".to_owned()),
            ..Default::default()
        },
    );
    assert!(
        t.contains("BGM+Z19+ANF-4711") && t.contains("IMD++Z01") && t.contains("CCI+Z02"),
        "{t}"
    );

    let t = unchecked(
        K::Lieferantenclearingliste,
        V::OhneAbo,
        NB,
        AnforderungAngaben {
            zeitintervall: monat(),
            version: version(),
            meldepunkt: Some(MABIS_ZP.to_owned()),
            ..Default::default()
        },
    );
    assert!(
        t.contains("DTM+273:202610:610") && t.contains("IMD++Z03"),
        "{t}"
    );
    assert!(t.contains("RFF+AUU:"), "{t}");

    let t = unchecked(
        K::Bilanzkreiszuordnungsliste,
        V::OhneAbo,
        NB,
        AnforderungAngaben {
            zeitintervall: monat(),
            bilanzierungsgebiet: Some("11YR00000000000U".to_owned()),
            bilanzkreis: Some("11XBK-STD------Y".to_owned()),
            ..Default::default()
        },
    );
    assert!(
        t.contains("BGM+E40")
            && t.contains("LOC+107+11YR00000000000U")
            && t.contains("LOC+237+11XBK-STD------Y"),
        "{t}"
    );

    let t = send(
        K::ClearinglisteBas,
        V::Bestellung,
        BIKO,
        AnforderungAngaben {
            zeitintervall: monat(),
            version: version(),
            meldepunkt: Some(MABIS_ZP.to_owned()),
            ..Default::default()
        },
    );
    assert!(t.contains("IMD++Z03") && t.contains("RFF+Z13:17204"), "{t}");

    let t = send(
        K::ClearinglisteDzr,
        V::Bestellung,
        BIKO,
        AnforderungAngaben {
            zeitintervall: monat(),
            version: version(),
            meldepunkt: Some(MABIS_ZP.to_owned()),
            ..Default::default()
        },
    );
    assert!(t.contains("RFF+Z13:17205"), "{t}");
}

/// **The MaBiS Anforderungen of Übersicht 2 render their columns.** ORDERS
/// AHB 1.1b Kap. 5.2 (S. 120–122) and 5.4 (S. 125–126):
///
/// - 17206 / 17210 `BGM+Z05`, `IMD` `Z01`/`Z02`/`Z03`, `DTM+203` beside
///   Z01/Z02 (`[33]`/`[34]`), `DTM+273` and `SG34 RFF+AUU` beside Z03 (`[1]`),
///   `SG38 LOC+172` the Zählpunkt of the BG-SZR resp. LF-AASZR;
/// - 17207 `BGM+BK`, `DTM+203`, `IMD` `Z01`/`Z02`, `SG38 LOC+237` the
///   Bilanzkreis;
/// - 17208 `BGM+Z05`, `DTM+273`, `IMD++Z03`, `SG34 RFF+AUU`, `SG38 LOC+172`
///   the Zählpunkt der DZR.
///
/// Every variant passes the gate, the 17206 / 17210 places conditioned on
/// `[1]`/`[33]`/`[34]` included.
#[test]
fn the_uebersicht_2_anforderungen_render_their_columns() {
    use mako_mabis::anforderung::{
        AbonnementVorgang as V, AnforderungAngaben, AnforderungCommand, AnforderungKind as K,
        MabisAnforderungWorkflow as W,
    };
    const UENB: &str = "9900000000034";
    let queue = |kind: K, vorgang: V, sender: &str, receiver: &str, angaben| {
        W::handle(
            &Default::default(),
            AnforderungCommand::SendAnforderung {
                kind,
                vorgang,
                receiver: MarktpartnerCode::new(receiver),
                sender: MarktpartnerCode::new(sender),
                message_ref: MessageRef::new("ANF-4712"),
                angaben,
            },
        )
        .expect("accepted")
        .outbox[0]
            .clone()
    };
    let monat = || Some("202610".to_owned());
    let version = || Some("20261105120000+00".to_owned());
    let tag = || Some("2026-12-01".to_owned());

    let t = sendable(&queue(
        K::BkSzrAggregationsebene,
        V::Bestellung,
        LF,
        UENB,
        AnforderungAngaben {
            ausfuehrungsdatum: tag(),
            bilanzkreis: Some("11XBK-STD------Y".to_owned()),
            ..Default::default()
        },
    ));
    assert!(
        t.contains("BGM+BK+ANF-4712")
            && t.contains("IMD++Z01")
            && t.contains("DTM+203:202611302300?+00:303")
            && t.contains("LOC+237+11XBK-STD------Y")
            && t.contains("RFF+Z13:17207"),
        "{t}"
    );

    let t = sendable(&queue(
        K::ClearinglisteUenbDzr,
        V::Bestellung,
        UENB,
        BIKO,
        AnforderungAngaben {
            zeitintervall: monat(),
            version: version(),
            meldepunkt: Some(MABIS_ZP.to_owned()),
            ..Default::default()
        },
    ));
    assert!(
        t.contains("BGM+Z05")
            && t.contains("IMD++Z03")
            && t.contains("DTM+273:202610:610")
            && t.contains("RFF+AUU:")
            && t.contains(&format!("LOC+172+{MABIS_ZP}"))
            && t.contains("RFF+Z13:17208"),
        "{t}"
    );

    for (kind, sender, receiver, pid) in [
        (K::Bilanzierungsgebietsclearingliste, NB, UENB, 17_206),
        (K::LieferantenausfallarbeitsClearingliste, LF, NB, 17_210),
    ] {
        let t = sendable(&queue(
            kind,
            V::Bestellung,
            sender,
            receiver,
            AnforderungAngaben {
                ausfuehrungsdatum: tag(),
                meldepunkt: Some(MABIS_ZP.to_owned()),
                ..Default::default()
            },
        ));
        assert!(
            t.contains("BGM+Z05")
                && t.contains("IMD++Z01")
                && t.contains("DTM+203:")
                && !t.contains("DTM+273")
                && t.contains(&format!("LOC+172+{MABIS_ZP}"))
                && t.contains(&format!("RFF+Z13:{pid}")),
            "{t}"
        );
        let t = sendable(&queue(
            kind,
            V::OhneAbo,
            sender,
            receiver,
            AnforderungAngaben {
                zeitintervall: monat(),
                version: version(),
                meldepunkt: Some(MABIS_ZP.to_owned()),
                ..Default::default()
            },
        ));
        assert!(
            t.contains("IMD++Z03")
                && t.contains("DTM+273:202610:610")
                && !t.contains("DTM+203")
                && t.contains("RFF+AUU:"),
            "{t}"
        );
    }
}

/// **A Reklamation names the reclaimed profile delivery.** ORDERS AHB 1.1b
/// Kap. 5.5 (S. 126–127): `BGM+Z58`, `FTX+AFH++<code>:E_0100`, `SG1 RFF+ACW`
/// the MSCONS' `BGM` DE 1004 (`[525]`).
#[test]
fn a_profil_reklamation_renders_a_sendable_17211() {
    use mako_engine::types::{BillingPeriod, Pruefidentifikator};
    use mako_mabis::profile::{MabisProfilWorkflow as W, ProfilCommand, ProfilState};
    let out = W::handle(
        &ProfilState::New,
        ProfilCommand::ReceiveProfile {
            pid: Pruefidentifikator::new(13_011).expect("valid"),
            sender: MarktpartnerCode::new(NB),
            receiver: MarktpartnerCode::new(LF),
            bilanzierungsmonat: BillingPeriod::new("2026-10"),
            version: 1,
            message_ref: MessageRef::new("MSCONS-UNH-1"),
            dokumentennummer: "PROFIL-0815".to_owned(),
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .expect("received");
    let state = out.events.iter().fold(ProfilState::New, W::apply);
    let out = W::handle(
        &state,
        ProfilCommand::SendReklamation {
            antwortcode: "A01".to_owned(),
            maengel: "Profil gehört zu keiner abonnierten Profilgruppe".to_owned(),
            message_ref: MessageRef::new("REK-1"),
        },
    )
    .expect("sent");
    let t = sendable(&out.outbox[0]);
    assert!(t.contains("BGM+Z58+REK-1"), "{t}");
    assert!(t.contains("FTX+AFH++A01:E_0100"), "{t}");
    assert!(t.contains("RFF+ACW:PROFIL-0815"), "{t}");
}
