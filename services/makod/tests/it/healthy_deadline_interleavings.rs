//! Property: a run in which every party answers in time never ends lapsed or
//! rejected, whatever order its deadlines fire in.
//!
//! Deadlines are never cancelled when a process moves on, so on the healthy
//! path every one of them fires — some before the next message, some long
//! after the process finished — interleaved with the delivery windows of the
//! acknowledgements that ride beside it. This drives the **pure**
//! `handle` / `apply` of each workflow through its healthy message sequence and,
//! for a range of seeds, fires every registered deadline at a random point
//! after the step that discharges it, plus delivery windows at random points
//! throughout. A fired deadline takes the production path: the label filter of
//! [`mako_engine::process::Process::run_timeout`], then the workflow's own
//! `on_deadline`, then `handle` and `apply`.
//!
//! The invariant: every healthy step is still accepted, and no state on the way
//! — let alone the last — is `Rejected`, `Abgelehnt`, `FristAbgelaufen` or
//! `DeadlineExpired`.

use std::fmt::Debug;

use mako_engine::{
    deadline::Deadline,
    ids::{ProcessId, StreamId, TenantId},
    lf_vorgang::LfVorgangsdaten,
    types::{DeviceId, MaLo, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::Workflow,
};
use time::macros::datetime;

/// Seeds per scenario.
const RUNS: u64 = 400;

/// Delivery windows that ride beside every process; none of them may ever
/// reach a workflow.
const DELIVERY_WINDOWS: &[&str] = &[
    mako_fristen::APERAK_STROM_WINDOW_LABEL,
    mako_fristen::APERAK_GAS_INITIALPROZESS_LABEL,
    mako_fristen::APERAK_GAS_FOLGEPROZESS_LABEL,
    mako_fristen::CONTRL_FRIST_LABEL,
];

const RECEIVED_AT: time::OffsetDateTime = datetime!(2026-03-02 08:00 UTC);

/// xorshift64* — deterministic, so a failing seed reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % (n as u64)).expect("fits")
    }
    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

/// One workflow's healthy run.
struct Scenario<W: Workflow> {
    name: &'static str,
    /// The healthy message sequence, each step in time.
    steps: Vec<W::Command>,
    /// Deadlines a `makod` door registers beside step `n`'s command.
    external: &'static [(&'static str, usize)],
    /// For every label the run registers, the step whose answer discharges it.
    discharged_by: &'static [(&'static str, usize)],
}

fn discharge_step(scenario_name: &str, table: &[(&str, usize)], label: &str) -> usize {
    table
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, s)| *s)
        .or_else(|| DELIVERY_WINDOWS.contains(&label).then_some(0))
        .unwrap_or_else(|| {
            panic!("{scenario_name}: the run registers {label:?}; say which step discharges it")
        })
}

fn is_lapsed_or_rejected(state: &impl Debug) -> bool {
    let shown = format!("{state:?}");
    [
        "Rejected",
        "Abgelehnt",
        "FristAbgelaufen",
        "DeadlineExpired",
    ]
    .iter()
    .any(|bad| shown.starts_with(bad))
}

/// The production path of one fired deadline, over the pure functions.
fn fire<W: Workflow>(state: W::State, label: &str) -> W::State {
    if !W::DEADLINE_LABELS.contains(&label) {
        // `Process::run_timeout` answers `None` without consulting the
        // hook; the dispatcher alerts on a delivery window instead.
        return state;
    }
    let deadline = Deadline::new(
        StreamId::new("process/simulated"),
        ProcessId::new(),
        TenantId::new(),
        WorkflowId::new("simulated", "FV2026-04-01"),
        label,
        RECEIVED_AT,
    );
    let Some(command) = W::on_deadline(&deadline, &state) else {
        return state;
    };
    let out = W::handle(&state, command).expect("a decided timeout is accepted");
    out.events.iter().fold(state, W::apply)
}

fn run<W>(scenario: &Scenario<W>)
where
    W: Workflow,
    W::Command: Clone,
    W::State: Debug,
{
    for seed in 1..=RUNS {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let mut state = W::State::default();
        // (label, earliest step before which it may fire)
        let mut pending: Vec<(String, usize)> = Vec::new();
        let n = scenario.steps.len();

        for (i, command) in scenario.steps.iter().enumerate() {
            // Deadlines whose answer is already in may fire now, in any order.
            pending.sort_by_key(|_| rng.next());
            let mut keep = Vec::new();
            for (label, from) in pending.drain(..) {
                if from <= i && rng.coin() {
                    state = fire::<W>(state, &label);
                } else {
                    keep.push((label, from));
                }
            }
            pending = keep;
            if rng.below(3) == 0 {
                let window = DELIVERY_WINDOWS[rng.below(DELIVERY_WINDOWS.len())];
                state = fire::<W>(state, window);
            }
            assert!(
                !is_lapsed_or_rejected(&state),
                "{}: seed {seed}: before step {i} the healthy run reached {state:?}",
                scenario.name
            );

            let out = W::handle(&state, command.clone()).unwrap_or_else(|e| {
                panic!(
                    "{}: seed {seed}: healthy step {i} refused in {state:?}: {e}",
                    scenario.name
                )
            });
            for d in &out.deadlines {
                let discharged = discharge_step(scenario.name, scenario.discharged_by, &d.label);
                pending.push((d.label.clone(), discharged.max(i) + 1));
            }
            for (label, at) in scenario.external {
                if *at == i {
                    let discharged = discharge_step(scenario.name, scenario.discharged_by, label);
                    pending.push(((*label).to_owned(), discharged.max(i) + 1));
                }
            }
            state = out.events.iter().fold(state, W::apply);
        }

        // Everything still registered fires after the run, in any order.
        pending.sort_by_key(|_| rng.next());
        for (label, from) in pending {
            assert!(
                from <= n,
                "{}: {label} discharged after the run",
                scenario.name
            );
            state = fire::<W>(state, &label);
        }
        for window in DELIVERY_WINDOWS {
            state = fire::<W>(state, window);
        }
        assert!(
            !is_lapsed_or_rejected(&state),
            "{}: seed {seed}: the healthy run ended {state:?}",
            scenario.name
        );
    }
}

fn mp(code: &str) -> MarktpartnerCode {
    MarktpartnerCode::new(code)
}
fn pid(code: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(code).expect("PID")
}

// ── GeLi Gas Lieferbeginn ────────────────────────────────────────────────────

#[test]
fn geli_gas_lieferbeginn_survives_every_interleaving() {
    use mako_geli_gas::{
        GasSupplierChangeCommand as C, GeliGasSupplierChangeWorkflow,
        LIEFERBEGINN_RESPONSE_WINDOW_LABEL as ANTWORT,
    };
    run(&Scenario::<GeliGasSupplierChangeWorkflow> {
        name: "GeLi Gas 44001 (GNB answers)",
        steps: vec![
            C::ReceiveUtilmd {
                pid: pid(44_001),
                sender: mp("4012345000023"),
                receiver: mp("9800357000004"),
                malo_id: MaLo::new("51238696012"),
                document_date: "20260302".to_owned(),
                process_date: "20260401".to_owned(),
                message_ref: MessageRef::new("GAS-1"),
                validation_passed: true,
                validation_errors: vec![],
                received_at: RECEIVED_AT,
                bilanzierungsmethode: None,
                vorgang: LfVorgangsdaten::default(),
                fallgruppe: None,
                gasqualitaet: None,
            },
            C::SendAntwort {
                accepted: true,
                antwort_code: "E15".to_owned(),
                bemerkung: None,
                obligations: vec![],
            },
            C::Activate,
        ],
        external: &[(ANTWORT, 0)],
        discharged_by: &[(ANTWORT, 1)],
    });
    run(&Scenario::<GeliGasSupplierChangeWorkflow> {
        name: "GeLi Gas 44013 (E/G answers the GNB)",
        steps: vec![
            C::InitiateGnbProcess {
                pid: pid(44_013),
                sender: mp("9800357000004"),
                receiver: mp("4012345000023"),
                malo_id: MaLo::new("51238696012"),
                document_date: "20260302".to_owned(),
                process_date: "20260401".to_owned(),
                message_ref: MessageRef::new("EOG-1"),
            },
            C::ReceiveGnbAntwort {
                response_pid: pid(44_014),
                accepted: true,
                reason: None,
            },
        ],
        external: &[(ANTWORT, 0)],
        discharged_by: &[(ANTWORT, 1)],
    });
}

// ── WiM Beginn Messstellenbetrieb ────────────────────────────────────────────

fn msb_anmeldung(code: u32) -> mako_wim::DeviceChangeCommand {
    mako_wim::DeviceChangeCommand::ReceiveUtilmd {
        pid: pid(code),
        sender: mp("4012345000030"),
        receiver: mp("9900357000004"),
        melo_id: MeLo::new("DE0001000001234567890000000000001"),
        device_id: DeviceId::new("ZHR-1"),
        document_date: "20260302".to_owned(),
        message_ref: MessageRef::new("WIM-1"),
        vorgangsnummer: Some("VG-1".to_owned()),
        process_date: Some("20260316".to_owned()),
        transaktionsgrund: Some("E03".to_owned()),
        validation_passed: true,
        validation_errors: vec![],
        received_at: RECEIVED_AT,
    }
}

fn wim_rest_of_beginn() -> Vec<mako_wim::DeviceChangeCommand> {
    use mako_wim::DeviceChangeCommand as C;
    vec![
        C::DispatchAntwort {
            bestaetigt: true,
            antwort_code: "E15".to_owned(),
            bemerkung: None,
            abweichender_termin: None,
        },
        C::ReceiveGesamtvorgang {
            pid: pid(mako_wim::geraetewechsel::GESAMTVORGANG_ERFOLG_PID),
            zuordnungsbeginn: Some("20260316".to_owned()),
            message_ref: MessageRef::new("GV-1"),
            received_at: datetime!(2026-03-17 08:00 UTC),
        },
        C::DispatchZuordnung { zugeordnet: true },
    ]
}

#[test]
fn wim_beginn_msb_survives_every_interleaving() {
    use mako_wim::geraetewechsel::{
        ANTWORT_FRIST_WINDOW_LABEL as ANTWORT, AUFTRAG_ANTWORT_WINDOW_LABEL as AUFTRAG,
        GESAMTVORGANG_AUSBLEIBEN_WINDOW_LABEL as AUSBLEIBEN,
        GESAMTVORGANG_MELDUNG_WINDOW_LABEL as MELDUNG, ZUORDNUNG_ANTWORT_WINDOW_LABEL as ZUORDNUNG,
    };
    use mako_wim::{DeviceChangeCommand as C, WimDeviceChangeWorkflow};

    let mut strom = vec![
        msb_anmeldung(55_042),
        C::DispatchAperak {
            positive: true,
            reason: None,
        },
    ];
    strom.extend(wim_rest_of_beginn());
    run(&Scenario::<WimDeviceChangeWorkflow> {
        name: "WiM 55042 (NB answers, MSBN reports)",
        steps: strom,
        external: &[],
        discharged_by: &[(ANTWORT, 2), (AUSBLEIBEN, 3), (ZUORDNUNG, 4)],
    });

    let mut gas = vec![msb_anmeldung(44_042)];
    gas.extend(wim_rest_of_beginn());
    run(&Scenario::<WimDeviceChangeWorkflow> {
        name: "WiM 44042 (NB answers, MSBN reports)",
        steps: gas,
        external: &[],
        discharged_by: &[(ANTWORT, 1), (AUSBLEIBEN, 2), (ZUORDNUNG, 3)],
    });

    run(&Scenario::<WimDeviceChangeWorkflow> {
        name: "WiM 55042 (MSBN orders, NB answers and assigns)",
        steps: vec![
            C::InitiateDeviceChange {
                pid: pid(55_042),
                sender: mp("4012345000030"),
                receiver: mp("9900357000004"),
                melo_id: MeLo::new("DE0001000001234567890000000000001"),
                process_date: "20260316".to_owned(),
                message_ref: MessageRef::new("WIM-OUT-1"),
            },
            C::ReceiveAntwort {
                pid: pid(mako_wim::geraetewechsel::antwort_pid_for(55_042, true)
                    .expect("55042 has a Bestätigung")),
                sender: mp("9900357000004"),
                message_ref: MessageRef::new("WIM-ANT-1"),
                reason: None,
                bestaetigter_termin: None,
            },
            C::MeldeGesamtvorgang {
                erfolgreich: true,
                zuordnungsbeginn: Some("20260316".to_owned()),
            },
            C::ReceiveZuordnungsantwort {
                pid: pid(mako_wim::geraetewechsel::ZUORDNUNG_ERFOLG_PID),
                zuordnungsbeginn: Some("20260316".to_owned()),
            },
        ],
        external: &[(AUFTRAG, 0)],
        discharged_by: &[(AUFTRAG, 1), (MELDUNG, 2)],
    });
}

// ── LF-Anmeldung, Strom and Gas ──────────────────────────────────────────────

#[test]
fn lf_anmeldung_survives_every_interleaving() {
    use mako_gpke::{
        GpkeLfAnmeldungWorkflow, LfAnmeldungCommand as C, NB_RESPONSE_WINDOW_LABEL as ANTWORT,
    };
    run(&Scenario::<GpkeLfAnmeldungWorkflow> {
        name: "GPKE 55001 (NB answers the LF)",
        steps: vec![
            C::InitiateAnmeldung {
                transaktionsgrund_ergaenzung: None,
                tranchengroesse: None,
                pid: pid(55_001),
                sender: mp("4012345000023"),
                receiver: mp("9900357000004"),
                location_id: MaLo::new("51238696012"),
                process_date: "2026-04-01".to_owned(),
                transaktionsgrund: None,
                bilanzkreis: Some("11XBK-LF-------9".to_owned()),
            },
            C::HandleAntwort {
                response_pid: pid(55_003),
                accepted: true,
                reason: None,
                response_ref: MessageRef::new("NB-1"),
            },
        ],
        external: &[(ANTWORT, 0)],
        discharged_by: &[(ANTWORT, 1)],
    });
}

// ── INVOIC settlement ────────────────────────────────────────────────────────

#[test]
fn invoic_settlement_survives_every_interleaving() {
    use mako_gpke::{ABRECHNUNG_WINDOW_LABEL as SETTLEMENT, GpkeAbrechnungWorkflow};
    use mako_invoic::InvoicCommand as C;
    run(&Scenario::<GpkeAbrechnungWorkflow> {
        name: "INVOIC 31001 (the payer settles)",
        steps: vec![
            C::ReceiveInvoic {
                pid: pid(31_001),
                sender: mp("9900357000004"),
                recipient: mp("4012345000023"),
                invoice_ref: MessageRef::new("INV-1"),
                document_date: "20260302".to_owned(),
                validation_passed: true,
                validation_errors: vec![],
                rechnung: None,
                bestellung_ref: None,
                rechnungstyp: None,
            },
            C::SettleInvoice {
                message_ref: MessageRef::new("REMADV-1"),
            },
        ],
        external: &[(SETTLEMENT, 0)],
        discharged_by: &[(SETTLEMENT, 1)],
    });
}

// ── Redispatch acknowledgement ───────────────────────────────────────────────

#[test]
fn redispatch_acknowledgement_survives_every_interleaving() {
    use mako_redispatch::ack_forward::{
        AckForwardCommand as C, VerfuegbarkeitWorkflow, verfuegbarkeit::ACK_WINDOW_LABEL as ACK,
    };
    run(&Scenario::<VerfuegbarkeitWorkflow> {
        name: "Redispatch Verfügbarkeit (acknowledged and forwarded)",
        steps: vec![
            C::Receive {
                mrid: "m1".into(),
                doc_type: "Unavailability".into(),
                sender: "s".into(),
                receiver: "r".into(),
                received_at: "2026-03-02T08:00:00Z".into(),
            },
            C::Acknowledge {
                ack_mrid: "ack-1".into(),
            },
            C::Forward {
                upstream_mrid: "up-1".into(),
            },
        ],
        external: &[(ACK, 0)],
        discharged_by: &[(ACK, 1)],
    });
}

/// The invariant check itself catches a lapsed run: a deadline fired before
/// its answer is a Frist breach, and the property would see it.
#[test]
fn the_property_sees_a_lapse() {
    use mako_geli_gas::{
        GasSupplierChangeCommand as C, GeliGasSupplierChangeWorkflow,
        LIEFERBEGINN_RESPONSE_WINDOW_LABEL as ANTWORT,
    };
    let out = GeliGasSupplierChangeWorkflow::handle(
        &Default::default(),
        C::ReceiveUtilmd {
            pid: pid(44_001),
            sender: mp("4012345000023"),
            receiver: mp("9800357000004"),
            malo_id: MaLo::new("51238696012"),
            document_date: "20260302".to_owned(),
            process_date: "20260401".to_owned(),
            message_ref: MessageRef::new("GAS-1"),
            validation_passed: true,
            validation_errors: vec![],
            received_at: RECEIVED_AT,
            bilanzierungsmethode: None,
            vorgang: LfVorgangsdaten::default(),
            fallgruppe: None,
            gasqualitaet: None,
        },
    )
    .unwrap();
    let state = out
        .events
        .iter()
        .fold(Default::default(), GeliGasSupplierChangeWorkflow::apply);
    let lapsed = fire::<GeliGasSupplierChangeWorkflow>(state, ANTWORT);
    assert!(is_lapsed_or_rejected(&lapsed), "{lapsed:?}");
}
