//! Outbox entries must reach the wire as EDIFACT, not as JSON.
//!
//! An outbox `message_type` with no renderer behind it does not fail loudly.
//! `render_to_wire_bytes` returns `InsufficientPayload`, and the AS4 sender
//! substitutes the raw domain **JSON** for the interchange: the message leaves
//! the system, looks delivered, and cannot be parsed by the receiving partner.
//! Only the in-process loopback path dead-letters.
//!
//! Workflow-level tests cannot see this. They assert that an outbox entry
//! exists and carries the expected fields, which it does — the mismatch is
//! between the producer's key names and the renderer's contract. These tests
//! cross that boundary: render the entry, then parse the bytes back.

use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::edifact_renderer::render_to_wire_bytes;
use makod::party_registry::MpIdRegistry;

const GNB: &str = "9870000000009";
const LFN: &str = "9871111111116";
const MALO: &str = "51238696012";
const MELO: &str = "DE0004096999000000000000000000009";

fn registry() -> MpIdRegistry {
    MpIdRegistry::from_config(&[
        PartyConfig {
            mp_id: GNB.to_owned(),
            roles: vec!["GNB".to_owned()],
            primary: true,
            agency: None,
        },
        PartyConfig {
            mp_id: LFN.to_owned(),
            roles: vec!["LF".to_owned()],
            primary: false,
            agency: None,
        },
    ])
    .expect("valid registry")
}

fn pid_of(bytes: &[u8]) -> u32 {
    use edi_energy::EdiEnergyMessage as _;
    edi_energy::parse(bytes)
        .expect("rendered bytes must parse as EDIFACT")
        .detect_pruefidentifikator()
        .expect("the rendered interchange must announce its Prüfidentifikator")
        .as_u32()
}

fn outbox(message_type: &str, recipient: &str, payload: serde_json::Value) -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("process/geli-gas-test"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        message_type,
        recipient,
        payload,
    )
}

/// The Bestätigung Anmeldung NN (44002) must render and re-parse as UTILMD.
#[test]
fn gnb_bestaetigung_renders_to_parseable_utilmd() {
    let msg = outbox(
        "UTILMD",
        LFN,
        serde_json::json!({
            "pid":           44002,
            "anfrage_pid":   44001,
            "accepted":      true,
            "malo":          MALO,
            "sender":        GNB,
            "receiver":      LFN,
            "document_date": "20261101",
            "process_date":  "20261101",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry())
        .expect("the GNB answer must have a renderer — it is a UTILMD interchange");

    assert_eq!(
        pid_of(&rendered.bytes),
        44002,
        "the wire message must carry the *answer* PID, not the Anfrage PID"
    );
}

/// The Ablehnung (44003) must render too — the rejection path is not special.
#[test]
fn gnb_ablehnung_renders_to_parseable_utilmd() {
    let msg = outbox(
        "UTILMD",
        LFN,
        serde_json::json!({
            "pid":           44003,
            "anfrage_pid":   44001,
            "accepted":      false,
            "reason":        "MaLo nicht im Netzgebiet",
            "malo":          MALO,
            "sender":        GNB,
            "receiver":      LFN,
            "document_date": "20261101",
            "process_date":  "20261101",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry()).expect("Ablehnung must render");
    assert_eq!(pid_of(&rendered.bytes), 44003);
}

/// A GNB-initiated Anfrage (44007 Abmeldung NN vom NB) must render.
#[test]
fn gnb_initiated_anfrage_renders_to_parseable_utilmd() {
    let msg = outbox(
        "UTILMD",
        LFN,
        serde_json::json!({
            "pid":           44007,
            "anfrage_pid":   44007,
            "malo":          MALO,
            "sender":        GNB,
            "receiver":      LFN,
            "document_date": "20261101",
            "process_date":  "20261101",
            "message_ref":   "MSG-001",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry()).expect("Anfrage must render");
    assert_eq!(pid_of(&rendered.bytes), 44007);
}

/// The old type names must stay unrenderable, so a reintroduction fails loudly.
///
/// This is the actual regression: the names looked plausible and the producer
/// compiled, but nothing downstream could act on them.
/// **The GeLi Gas LF-Stornierung must reach the wire.**
///
/// UTILMD Gas 44022 (LFN/LFA → GNB, „Stornierung einer Anmeldung/Abmeldung",
/// GeLi Gas 3.0). The outbox entry comes out of the workflow itself rather
/// than being hand-built here: a payload written to one set of key names and a
/// renderer reading another is the exact mismatch this file exists to catch,
/// and only crossing the boundary with the producer's own output can see it.
#[test]
fn the_lf_stornierung_renders_to_parseable_utilmd() {
    use mako_engine::types::{MaLo, MarktpartnerCode, Pruefidentifikator};
    use mako_engine::workflow::Workflow;
    use mako_geli_gas::{GeliGasLfStornierungWorkflow, LfStornierungCommand, LfStornierungState};

    let out = GeliGasLfStornierungWorkflow::handle(
        &LfStornierungState::New,
        LfStornierungCommand::InitiateStornierung {
            pid: Pruefidentifikator::const_new(44_022),
            sender: MarktpartnerCode::new(LFN),
            receiver: MarktpartnerCode::new(GNB),
            malo: MaLo::new(MALO),
            bgm_qualifier: "E01".to_owned(),
        },
    )
    .expect("the Stornierung is accepted");

    let entry = out.outbox.first().expect("the Stornierung is sent");
    let msg = outbox(&entry.message_type, GNB, entry.payload.clone());
    let wire = render_to_wire_bytes(&msg, &registry())
        .expect("the 44022 must render, not fall back to raw JSON");
    let text = String::from_utf8_lossy(&wire.bytes);

    assert_eq!(pid_of(&wire.bytes), 44_022);
    // `BGM` DE 1001 names the Anwendungsfall being cancelled, and `SG5 LOC+172`
    // the Meldepunkt — UTILMD AHB Gas uses `172` for every Lokation.
    assert!(text.contains("BGM+E01+44022"), "{text}");
    assert!(text.contains(&format!("LOC+172+{MALO}")), "{text}");
}

#[test]
fn the_old_intent_only_type_names_have_no_renderer() {
    for stale in ["UtilmdAnfrage", "UtilmdAntwort"] {
        let msg = outbox(
            stale,
            LFN,
            serde_json::json!({ "pid": 44002, "malo": MALO, "sender": GNB }),
        );
        assert!(
            render_to_wire_bytes(&msg, &registry()).is_err(),
            "{stale} must not gain a renderer — GeLi Gas emits plain `UTILMD` now, \
             and a message type only this crate understands cannot reach a partner"
        );
    }
}

// ── INSRPT ────────────────────────────────────────────────────────────────────

/// The WiM Störungsmeldung must render to an interchange that passes AHB
/// validation — not merely one that parses.
///
/// `mako-wim` emitted `INSRPT` outbox entries with no renderer behind them, so
/// the AS4 sender substituted raw domain JSON. Adding a renderer is only half
/// the fix: the INSRPT AHB marks `BGM`, `DOC`, `DTM`, `LIN`, `LOC`, `NAD`,
/// `RFF` and `STS` mandatory, and the builder emitted only three of them. This
/// asserts the full set by validating the result.
#[test]
fn wim_stoerungsmeldung_renders_to_ahb_valid_insrpt() {
    use edi_energy::EdiEnergyMessage as _;

    let msg = outbox(
        "INSRPT",
        LFN,
        serde_json::json!({
            "type":          "Stoerungsmeldung",
            "pid":           23001,
            "melo":          "DE00056266802AO6G56M11SN51G",
            "receiver":      LFN,
            "document_date": "20261101",
            "message_ref":   "MSG-INSRPT-1",
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry())
        .expect("INSRPT must have a renderer — mako-wim enqueues it");

    assert_eq!(pid_of(&rendered.bytes), 23001);

    let parsed = edi_energy::parse(&rendered.bytes).expect("rendered INSRPT must parse");
    let report = parsed
        .validate_on_date(time::macros::date!(2026 - 11 - 01))
        .expect("validation runs");
    assert!(
        report.is_valid(),
        "rendered INSRPT must satisfy the AHB, got: {:?}",
        report
            .iter_issues()
            .map(|i| format!(
                "{:?} {} {}",
                i.severity,
                i.rule_id.clone().unwrap_or_default(),
                i.message
            ))
            .collect::<Vec<_>>()
    );
}

// ── MSCONS SG6 LOC qualifiers ─────────────────────────────────────────────────

/// The MaBiS Summenzeitreihe must name its Meldepunkt and its
/// Bilanzierungsgebiet under **different** LOC qualifiers.
///
/// MSCONS AHB 3.2 gives PIDs 13003/13023 three SG6 LOC qualifiers: `172`
/// Meldepunkt (the MaBiS-Zählpunkt), `107` Bilanzierungsgebiet, `237`
/// Bilanzkreis. mako emitted the Bilanzierungsgebiet EIC under `172` and no
/// `107` at all — telling the BIKO a 16-character territory code was the
/// Meldepunkt, and omitting the territory. The message still parsed and still
/// validated, because both fields are free text at the MIG level.
#[test]
fn summenzeitreihe_separates_meldepunkt_from_bilanzierungsgebiet() {
    const MABIS_ZP: &str = "DE0004030099000000000000000012345";
    const BILANZIERUNGSGEBIET: &str = "11YAPG4CTRDNZ--P";

    let msg = outbox(
        "MSCONS",
        LFN,
        serde_json::json!({
            "pid": 13003,
            "mabis_zp_id": MABIS_ZP,
            "bilanzierungsgebiet_id": BILANZIERUNGSGEBIET,
            "balancing_period": "202606",
            "version": "20260714050000+00",
            "sender_mp_id": GNB,
            "receiver_mp_id": LFN,
            "intervals": [
                { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "7.5" },
            ],
        }),
    );

    let rendered = render_to_wire_bytes(&msg, &registry()).expect("13003 must render");
    let wire = String::from_utf8(rendered.bytes).expect("utf-8");

    assert!(
        wire.contains(&format!("LOC+172+{MABIS_ZP}")),
        "LOC+172 must carry the MaBiS-Zählpunkt:\n{wire}"
    );
    assert!(
        wire.contains(&format!("LOC+107+{BILANZIERUNGSGEBIET}")),
        "LOC+107 must carry the Bilanzierungsgebiet:\n{wire}"
    );
    assert!(
        !wire.contains(&format!("LOC+172+{BILANZIERUNGSGEBIET}")),
        "the Bilanzierungsgebiet must never appear as the Meldepunkt:\n{wire}"
    );
}

/// Passing the same value for both is refused at the boundary.
///
/// That is exactly the original defect — one identifier standing in for two —
/// and it is silent on the wire, so it has to fail before rendering.
#[test]
fn the_same_identifier_cannot_serve_as_both_loc_qualifiers() {
    const BOTH: &str = "11YAPG4CTRDNZ--P";
    let msg = outbox(
        "MSCONS",
        LFN,
        serde_json::json!({
            "pid": 13003,
            "mabis_zp_id": BOTH,
            "bilanzierungsgebiet_id": BOTH,
            "balancing_period": "202606",
            "version": "20260714050000+00",
            "intervals": [
                { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "7.5" },
            ],
        }),
    );
    assert!(
        render_to_wire_bytes(&msg, &registry()).is_err(),
        "the territory EIC standing in for the Meldepunkt must be refused"
    );
}

/// The rendered Summenzeitreihe must still satisfy the AHB.
///
/// The shipped MSCONS profile restricts SG6 `LOC` DE3227 to `172` — it was
/// imported before the `107`/`237` qualifiers were noticed — so emitting the
/// Bilanzierungsgebiet under `107` could be rejected by mako's own validator
/// even though the AHB permits it.
#[test]
fn the_rendered_summenzeitreihe_still_validates() {
    use edi_energy::EdiEnergyMessage as _;
    let msg = outbox(
        "MSCONS",
        LFN,
        serde_json::json!({
            "pid": 13003,
            "mabis_zp_id": "DE0004030099000000000000000012345",
            "bilanzierungsgebiet_id": "11YAPG4CTRDNZ--P",
            "balancing_period": "202606",
            "version": "20260714050000+00",
            "sender_mp_id": GNB,
            "receiver_mp_id": LFN,
            "intervals": [
                { "from": "202606010000+00", "to": "202606010015+00", "quantity_kwh": "7.5" },
            ],
        }),
    );
    let rendered = render_to_wire_bytes(&msg, &registry()).expect("13003 must render");
    let parsed = edi_energy::parse(&rendered.bytes).expect("must parse");
    let report = parsed
        .validate_on_date(time::macros::date!(2026 - 06 - 30))
        .expect("validation runs");
    assert!(
        report.is_valid(),
        "rendered 13003 must satisfy the AHB, got: {:?}",
        report
            .iter_issues()
            .map(|i| format!(
                "{:?} {} {}",
                i.severity,
                i.rule_id.clone().unwrap_or_default(),
                i.message
            ))
            .collect::<Vec<_>>()
    );
}

/// **A WiM Ersteinbau status message is not a 21042.**
///
/// IFTSTA AHB: `LOC` is Muss on 21029/21030/21031, and each Anwendungsfall has
/// its own `BGM` Dokumentenart and `STS`. A renderer that shapes every IFTSTA
/// as the UC-4.4 Umsetzungsstatus (21042: `BGM+Z09`, `STS+Z21+105`
/// „Bestellung beendet") emits a message about a Bestellung when the workflow
/// meant a Vorabinformation zum Ersteinbau, and drops the Messlokation the AHB
/// marks Muss.
#[test]
fn an_ersteinbau_status_names_its_messlokation_and_its_own_anwendungsfall() {
    use mako_engine::types::{MarktpartnerCode, MeLo};
    use mako_engine::workflow::Workflow;
    use mako_wim::ersteinbau::{ErsteinbauCommand, ErsteinbauState, WimErsteinbauWorkflow};

    let out = WimErsteinbauWorkflow::handle(
        &ErsteinbauState::New,
        ErsteinbauCommand::SendVorabinformation {
            gmsb: MarktpartnerCode::new(GNB),
            wmsb: MarktpartnerCode::new(LFN),
            melo_id: MeLo::new(MELO),
            umstellungszeitpunkt: "20270201".to_owned(),
            message_ref: mako_engine::types::MessageRef::new("MSG-1"),
        },
    )
    .expect("the Vorabinformation is accepted");

    let entry = out
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "IFTSTA")
        .expect("the Vorabinformation is an IFTSTA");
    let msg = outbox("IFTSTA", LFN, entry.payload.clone());
    let wire = render_to_wire_bytes(&msg, &registry()).expect("renders");
    let text = String::from_utf8_lossy(&wire.bytes);

    // IFTSTA AHB 2.1 Kap. 6.7, the 21029 column.
    assert!(text.contains("BGM+Z09"), "{text}");
    assert!(
        text.contains(&format!("LOC+172+{MELO}")),
        "SG14 LOC+172 is Muss and carries the Zählpunktbezeichnung:\n{text}"
    );
    assert!(
        text.contains("STS+Z19+:Z17"),
        "the Planungsstatus is „Ersteinbau iMS / geplant\u{201c}, not \u{201e}Bestellung / beendet\u{201c}:\n{text}"
    );
    assert!(
        text.contains("DTM+76:20270201:102"),
        "SG15 DTM+76 carries the planned Umstellungszeitpunkt in CCYYMMDD:\n{text}"
    );
    assert!(text.contains("RFF+Z13:21029"), "{text}");
}

/// The `E_0233` answers name their Prüfschritt and their cluster's own status.
///
/// 21030 „zugestimmt" (`Z30`) and 21031 „widersprochen" (`Z31`) — IFTSTA AHB
/// 2.1 Kap. 6.7 pins one DE 4405 per Anwendungsfall, and Bedingungen
/// `[47]`/`[48]` pin the DE 9013 code to that Anwendungsfall's EBD cluster.
#[test]
fn the_ersteinbau_answers_carry_their_status_and_their_e_0233_code() {
    for (pid, sts, code) in [(21_030_u32, "Z30", "A01"), (21_031, "Z31", "A04")] {
        let msg = outbox(
            "IFTSTA",
            GNB,
            serde_json::json!({
                "pid": pid,
                "sender": LFN,
                "receiver": GNB,
                "melo": MELO,
                "antwort_code": code,
                "antwort_codeliste": "E_0233",
            }),
        );
        let wire = render_to_wire_bytes(&msg, &registry()).expect("renders");
        let text = String::from_utf8_lossy(&wire.bytes);
        assert!(
            text.contains(&format!("STS+Z19+:{sts}+{code}:E_0233")),
            "{pid} wants STS+Z19+:{sts}+{code}:E_0233:\n{text}"
        );
        assert!(text.contains(&format!("LOC+172+{MELO}")), "{text}");
    }
}

/// **`SG4 STS+E01` DE 1131 has one payload key across every domain crate.**
///
/// The renderers read `antwort_codeliste` and nothing else. A domain crate that
/// spells the wire value differently loses DE 1131 silently: the answer still
/// renders, still parses, and no longer says which Entscheidungsbaum produced
/// its code — which is the only disambiguator where one answer PID carries
/// several trees (55003 carries `E_0622` and `E_0623`, 55239 carries `E_0510`
/// and `E_0513`, ORDRSP 19005/19006 carry four).
///
/// `antwort_tree` is the other half of the pair and deliberately *not* a wire
/// key: it records which tree resolved the code, which for Gas — whose
/// Codelisten the MIG does not name in the segment — is the only thing there is.
#[test]
fn de_1131_is_spelled_one_way_across_the_workspace() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");

    let mut offenders = Vec::new();
    for dir in ["crates", "services"] {
        let mut stack = vec![root.join(dir)];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|n| n == "target") {
                        continue;
                    }
                    stack.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") {
                    let Ok(src) = std::fs::read_to_string(&path) else {
                        continue;
                    };
                    if src.contains("\"antwort_ebd\"") {
                        offenders.push(
                            path.strip_prefix(&root)
                                .unwrap_or(&path)
                                .display()
                                .to_string(),
                        );
                    }
                }
            }
        }
    }
    offenders.sort();
    assert!(
        offenders.is_empty(),
        "these files spell the DE 1131 payload key \"antwort_ebd\"; the renderers read \
         \"antwort_codeliste\", so the Codeliste is dropped on the way to the wire:\n  {}",
        offenders.join("\n  ")
    );
}

/// **A MaBiS-ZP lifecycle answer must reach the wire.**
///
/// UTILMD 55064 answers the Aktivierung/Deaktivierung of a MaBiS-Zählpunkt
/// (BK6-24-174 Anlage 3). The object it names is a **MaBiS-Zählpunkt**, not a
/// Marktlokation, so it rides `SG5 LOC+Z15` — and the Antwortcode is read
/// against the tree that published it, of which 55064 has twelve.
#[test]
fn a_mabis_zp_answer_renders_to_parseable_utilmd() {
    use mako_engine::types::{MarktpartnerCode, Pruefidentifikator};
    use mako_engine::workflow::Workflow;
    use mako_mabis::zp_lifecycle::{
        MabisZpLifecycleWorkflow, ZpLifecycleCommand, ZpLifecycleState,
    };

    const ZP: &str = "DE0004096999000000000000000000009";

    let received = ZpLifecycleCommand::ReceiveAnfrage {
        pid: Pruefidentifikator::const_new(55_062),
        serie: mako_mabis::ZpSerie::NetzzeitreiheNachbarNb,
        vorgang: mako_mabis::zp_lifecycle::ZpVorgang::Aktivierung,
        mabis_zp_id: ZP.to_owned(),
        sender: MarktpartnerCode::new(GNB),
        receiver: MarktpartnerCode::new(LFN),
        document_date: "20270101".to_owned(),
        billing_period: mako_engine::types::BillingPeriod::new("202701"),
        message_ref: mako_engine::types::MessageRef::new("MSG-1"),
        validation_passed: true,
        validation_errors: Vec::new(),
    };
    let opened = MabisZpLifecycleWorkflow::handle(&ZpLifecycleState::New, received)
        .expect("the Anfrage is accepted");
    let state = opened
        .events
        .iter()
        .fold(ZpLifecycleState::New, MabisZpLifecycleWorkflow::apply);

    let out = MabisZpLifecycleWorkflow::handle(
        &state,
        ZpLifecycleCommand::SendAntwort {
            bestaetigt: true,
            grund: None,
        },
    )
    .expect("the answer is accepted");

    let entry = out
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "UTILMD")
        .expect("the answer is a UTILMD");
    let msg = outbox("UTILMD", GNB, entry.payload.clone());
    let wire = render_to_wire_bytes(&msg, &registry())
        .expect("the MaBiS-ZP answer must render, not fall back to raw JSON");
    let text = String::from_utf8_lossy(&wire.bytes);
    assert!(
        text.contains(&format!("LOC+Z15+{ZP}")),
        "a MaBiS-Zählpunkt rides LOC+Z15, never LOC+Z16:\n{text}"
    );
}

/// **A MaBiS Korrekturliste renders as an `IDE+Z01` list.**
///
/// UTILMD AHB Strom 2.2 Kap. 13.4: the head *is* the Geschäftsvorfall
/// („Alle aufgelisteten IDE+24 sind Bestandteil des Geschäftsvorfalls",
/// Bedingung `[564]`) and each disputed Marktlokation is a member under it.
/// Rendering the head alone would state „keine Korrekturen" — the
/// reconciliation the sender did not give.
#[test]
fn a_mabis_korrekturliste_renders_as_a_list_with_its_positions() {
    use mako_engine::types::MarktpartnerCode;
    use mako_engine::workflow::Workflow;
    use mako_mabis::listenabgleich::{
        ListenabgleichCommand, ListenabgleichState, MabisListenabgleichWorkflow,
    };
    use mako_pruefung::mabis::{Korrekturgrund, Korrekturposition};

    const ZP: &str = "DE0004096999000000000000000000009";
    const MALO_2: &str = "51238696781";

    let opened = MabisListenabgleichWorkflow::handle(
        &ListenabgleichState::New,
        ListenabgleichCommand::ReceiveListe {
            pid: mako_engine::types::Pruefidentifikator::const_new(55_065),
            mabis_zaehlpunkt: ZP.to_owned(),
            zeitreihen_version: "20270115T090000000".to_owned(),
            listennummer: "LST-1".to_owned(),
            sender: MarktpartnerCode::new(GNB),
            receiver: MarktpartnerCode::new(LFN),
            billing_period: mako_engine::types::BillingPeriod::new("202701"),
            message_ref: mako_engine::types::MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: Vec::new(),
        },
    )
    .expect("the list is accepted");
    let state = opened
        .events
        .iter()
        .fold(ListenabgleichState::New, MabisListenabgleichWorkflow::apply);

    let out = MabisListenabgleichWorkflow::handle(
        &state,
        ListenabgleichCommand::SendKorrektur {
            positionen: vec![
                Korrekturposition {
                    malo: MALO.to_owned(),
                    grund: Korrekturgrund::Entfallen,
                },
                Korrekturposition {
                    malo: MALO_2.to_owned(),
                    grund: Korrekturgrund::FalscheZuordnung,
                },
            ],
            sender_rolle: "NB".to_owned(),
        },
    )
    .expect("the Korrekturliste is accepted");

    let entry = out
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "UTILMD")
        .expect("the Korrekturliste is a UTILMD");
    let msg = outbox("UTILMD", GNB, entry.payload.clone());
    let wire = render_to_wire_bytes(&msg, &registry())
        .expect("the Korrekturliste must render, not fall back to raw JSON");
    let text = String::from_utf8_lossy(&wire.bytes);

    // Message level: `BGM+Z05` Clearingliste and the Bilanzierungsmonat in 610.
    assert!(text.contains("BGM+Z05+55066"), "{text}");
    assert!(text.contains("DTM+157:202701:610"), "{text}");
    // The head: the list, its Zählpunkt, the PID, the answered list's number,
    // and the Version der Zeitreihe.
    assert!(text.contains("IDE+Z01+LST-1-K"), "{text}");
    assert!(text.contains(&format!("LOC+Z15+{ZP}")), "{text}");
    assert!(text.contains("RFF+Z13:55066"), "{text}");
    assert!(text.contains("RFF+TN:LST-1"), "{text}");
    assert!(text.contains("SEQ+Z22"), "{text}");
    assert!(text.contains("RFF+AUU:20270115T090000000"), "{text}");
    // Both members, each with its own Antwortcode and Marktlokation.
    assert_eq!(
        text.matches("IDE+24+").count(),
        2,
        "one Vorgang per position:\n{text}"
    );
    assert!(text.contains(&format!("LOC+Z16+{MALO}")), "{text}");
    assert!(text.contains(&format!("LOC+Z16+{MALO_2}")), "{text}");
    assert_eq!(
        text.matches("STS+E01++").count(),
        2,
        "the Antwortcode is per position, not per list:\n{text}"
    );
    assert!(text.contains(":E_0047"), "DE 1131 names the tree:\n{text}");
    // The head carries no `STS+E01` of its own — Bedingung [238] makes the two
    // mutually exclusive, and a head status means the whole list was refused.
    assert!(
        !text.contains("IDE+Z01+LST-1-K'STS+E01"),
        "a list with positions carries no head status:\n{text}"
    );
    // It parses back.
    assert_eq!(pid_of(&wire.bytes), 55_066);
}

/// **A Gesamtablehnung renders: it names no Marktlokation by design.**
///
/// The cluster that carries no positions — the Abonnement was never ordered,
/// the version is not admitted, the Zeitraum is implausible. It refuses the
/// whole list, so there is nothing per-Marktlokation to say, and the single
/// Vorgang this renderer emits is the right shape for it.
#[test]
fn a_mabis_gesamtablehnung_renders_to_parseable_utilmd() {
    use mako_engine::types::MarktpartnerCode;
    use mako_engine::workflow::Workflow;
    use mako_mabis::listenabgleich::{
        ListenabgleichCommand, ListenabgleichState, MabisListenabgleichWorkflow,
    };

    let opened = MabisListenabgleichWorkflow::handle(
        &ListenabgleichState::New,
        ListenabgleichCommand::ReceiveListe {
            pid: mako_engine::types::Pruefidentifikator::const_new(55_065),
            mabis_zaehlpunkt: "DE0004096999000000000000000000009".to_owned(),
            zeitreihen_version: "20270115T090000000".to_owned(),
            listennummer: "LST-1".to_owned(),
            sender: MarktpartnerCode::new(GNB),
            receiver: MarktpartnerCode::new(LFN),
            billing_period: mako_engine::types::BillingPeriod::new("202701"),
            message_ref: mako_engine::types::MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: Vec::new(),
        },
    )
    .expect("the list is accepted");
    let state = opened
        .events
        .iter()
        .fold(ListenabgleichState::New, MabisListenabgleichWorkflow::apply);

    let out = MabisListenabgleichWorkflow::handle(
        &state,
        ListenabgleichCommand::SendGesamtAblehnung {
            // `E_0004` — the tree a 55065 is answered out of when the **ÜNB**
            // distributed it, and the only one of the pair with a whole-list
            // Abonnement Prüfschritt.
            sender_rolle: "ÜNB".to_owned(),
            abonnement_bestellt: Some(false),
            zeitraum_plausibel: Some(true),
            mabis_zp_passt: Some(true),
            version_zugelassen: Some(true),
            innerhalb_clearingphase: Some(true),
        },
    )
    .expect("the Gesamtablehnung is accepted");

    let entry = out
        .outbox
        .iter()
        .find(|o| o.message_type.as_ref() == "UTILMD")
        .expect("the Gesamtablehnung is a UTILMD");
    let msg = outbox("UTILMD", GNB, entry.payload.clone());
    let wire = render_to_wire_bytes(&msg, &registry())
        .expect("a positionless Ablehnung must render, not fall back to raw JSON");
    let text = String::from_utf8_lossy(&wire.bytes);
    assert!(
        text.contains("STS+E01++"),
        "the Antwortcode is on the wire:\n{text}"
    );
}
