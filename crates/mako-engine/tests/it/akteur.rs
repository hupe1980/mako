//! Every appended event names who caused it: a fired deadline is the system's
//! act, a command carries the actor its door stated, and the stored envelope
//! keeps it through a reload.

use mako_engine::{
    deadline::Deadline,
    envelope::Eingriffsklasse,
    envelope::{Akteur, EventEnvelope, Vorschlag},
    error::WorkflowError,
    event_store::{EventStore, InMemoryEventStore},
    ids::TenantId,
    outbox::PendingOutbox,
    process::{Process, RunOptions},
    version::WorkflowId,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

const FRIST: &str = "test-frist";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
enum Ev {
    Gestartet,
    Abgelaufen,
}

impl EventPayload for Ev {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Gestartet => "Gestartet",
            Self::Abgelaufen => "Abgelaufen",
        }
    }
}

#[derive(Debug, Clone)]
enum Cmd {
    Start,
    Ablauf,
}

impl CommandPayload for Cmd {}

struct Wf;

impl Workflow for Wf {
    type State = bool;
    type Event = Ev;
    type Command = Cmd;

    const DEADLINE_LABELS: &'static [&'static str] = &[FRIST];

    fn apply(_state: bool, event: &Ev) -> bool {
        matches!(event, Ev::Abgelaufen)
    }

    fn handle(_state: &bool, command: Cmd) -> Result<WorkflowOutput<Ev>, WorkflowError> {
        Ok(match command {
            Cmd::Start => vec![Ev::Gestartet].into(),
            Cmd::Ablauf => WorkflowOutput {
                events: vec![Ev::Abgelaufen],
                outbox: vec![PendingOutbox::new(
                    "AntwortfristAbgelaufen",
                    "9900357000004",
                    serde_json::json!({}),
                )],
                deadlines: vec![],
            },
        })
    }

    fn on_deadline(_deadline: &Deadline, state: &bool) -> Option<Cmd> {
        (!state).then_some(Cmd::Ablauf)
    }
}

#[tokio::test]
async fn an_appended_event_carries_its_actor() {
    let store = InMemoryEventStore::new();
    let process = Process::<Wf, _>::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("akteur", "FV2025-10-01"),
    );
    let person = Akteur::Person {
        principal: "desk-user-7".to_owned(),
    };
    process
        .run(Cmd::Start, RunOptions::by(person.clone()))
        .await
        .expect("command");

    let deadline = Deadline::new(
        process.stream_id().clone(),
        process.process_id(),
        process.tenant_id(),
        process.workflow_id().clone(),
        FRIST,
        time::OffsetDateTime::UNIX_EPOCH,
    );
    // A fired deadline is the system's act, whatever the options name.
    let lapsed = process
        .run_timeout(&deadline, RunOptions::by(person.clone()))
        .await
        .expect("timeout")
        .expect("the window was open");

    let loaded = store.load(process.stream_id()).await.expect("load");
    assert_eq!(
        loaded[0].akteur, person,
        "the command's events name its person"
    );
    assert_eq!(
        loaded[1].akteur,
        Akteur::System,
        "a timeout is the system's"
    );

    // The timeout's message reports the process's most human actor so far:
    // a person touched it, so it did not run alone.
    assert_eq!(lapsed.outbox.len(), 1);
    assert_eq!(lapsed.outbox[0].akteur, Eingriffsklasse::Person);
}

#[test]
fn an_envelope_without_an_actor_does_not_load() {
    let freigabe = Akteur::Freigabe {
        principal: "sb-1".to_owned(),
        vorschlag: Vorschlag::Regel {
            name: "anmeldung-pruefung".to_owned(),
        },
        unveraendert: true,
        dienst: "processd".to_owned(),
    };
    let process = mako_engine::ids::ProcessId::new();
    let tenant = TenantId::new();
    let ctx = mako_engine::workflow::CommandContext::new(
        tenant,
        process,
        WorkflowId::new("akteur", "FV2025-10-01"),
    );
    let envelope = EventEnvelope::from_new(
        ctx.new_event(&Ev::Gestartet, freigabe.clone())
            .expect("event"),
        mako_engine::ids::StreamId::for_process(tenant, &process),
        1,
        time::OffsetDateTime::UNIX_EPOCH,
    );
    let mut json = serde_json::to_value(&envelope).expect("serialise");
    let stated = json
        .as_object_mut()
        .expect("object")
        .remove("akteur")
        .expect("the actor is serialised");
    assert!(
        serde_json::from_value::<EventEnvelope>(json.clone()).is_err(),
        "no actor is no event — there is no Unbekannt"
    );
    let mut with = json;
    with["akteur"] = stated;
    let read: EventEnvelope = serde_json::from_value(with).expect("loads with an actor");
    assert_eq!(read.akteur, freigabe);
    assert_eq!(read.akteur.klasse().as_str(), "freigabe");
}

#[test]
fn an_approval_assertion_names_no_service_and_no_unknown_field() {
    use mako_engine::envelope::Freigabeauftrag;
    let asserted = serde_json::json!({
        "principal": "sb-1",
        "vorschlag": { "art": "regel", "name": "nb-module" },
        "unveraendert": true,
    });
    let auftrag: Freigabeauftrag = serde_json::from_value(asserted.clone()).expect("an approval");
    assert_eq!(
        auftrag.durch("processd").expect("valid"),
        Akteur::Freigabe {
            principal: "sb-1".into(),
            vorschlag: Vorschlag::Regel {
                name: "nb-module".into()
            },
            unveraendert: true,
            dienst: "processd".into(),
        },
        "the relaying service is the door's to name"
    );
    let mut forged = asserted.clone();
    forged["dienst"] = "someone-else".into();
    assert!(serde_json::from_value::<Freigabeauftrag>(forged).is_err());
    let mut extra = asserted.clone();
    extra["vorschlag"]["extra"] = 1.into();
    assert!(serde_json::from_value::<Freigabeauftrag>(extra).is_err());
    let mut long = asserted;
    long["principal"] = "x".repeat(257).into();
    let long: Freigabeauftrag = serde_json::from_value(long).expect("parses");
    assert!(
        long.durch("processd").is_err(),
        "an over-long principal is refused"
    );
}
