//! Following a run and stopping it: `GET /api/v1/runs/{run}/schritte` streams
//! the run's reads from its journal, each under its journal sequence, then the
//! answer the answer read returns; a reconnect continues after its
//! `Last-Event-ID`; `POST …/stopp` stops it so no service is read after the
//! stop. Only the person who started the run follows or stops it here.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentplane::model::{Completion, ToolCall, Usage};
use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse as _;
use futures_util::StreamExt as _;
use mako_service::oidc::{Claims, JwtClaims};
use serde_json::{Value, json};

use agentd::handlers::{self, AppState};
use agentd::plane::{Activation, Plane, PlaneConfig, Stores};

const TENANT: &str = "9900357000004";

/// makod, answering every read; the call to `blocks`, when set, waits for
/// `release` after announcing itself on `entered`.
#[derive(Debug, Default)]
struct Makod {
    calls: Mutex<Vec<String>>,
    blocks: Option<&'static str>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl ToolClient for Makod {
    fn destination(&self, _tool: &ToolId) -> agentplane::tools::Destination {
        agentplane::tools::Destination::Local
    }

    async fn call(
        &self,
        tool: &ToolId,
        _arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        self.calls.lock().unwrap().push(tool.tool.clone());
        if self.blocks == Some(tool.tool.as_str()) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(json!({ "business_key": "51238696012", "processes": [] }))
    }
}

fn turn(structured: Option<Value>, tool_calls: Vec<ToolCall>) -> Completion {
    Completion {
        text: String::new(),
        model: None,
        structured,
        stop_reason: Some(
            if tool_calls.is_empty() {
                "end_turn"
            } else {
                "tool_use"
            }
            .to_owned(),
        ),
        tool_calls,
        usage: Usage::default(),
        truncated: false,
        continuation: None,
        ..Completion::default()
    }
}

fn read(id: &str, tool: &str, arguments: Value) -> Completion {
    turn(
        None,
        vec![ToolCall {
            id: id.to_owned(),
            name: format!("makod__{tool}"),
            arguments,
        }],
    )
}

fn answered() -> Completion {
    turn(
        Some(json!({ "saetze": [
            { "text": "Für 51238696012 läuft kein Prozess.", "quelle": "makod/get_process" }
        ], "vorschlag": null })),
        Vec::new(),
    )
}

fn state(tools: Arc<Makod>, provider: &Arc<FakeProvider>) -> Arc<AppState> {
    let tenant = agentplane::core::TenantId::new(TENANT).expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let client: Arc<dyn ToolClient> = tools;
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| (name, Arc::clone(&client)))
        .collect();
    let plane = Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-schritte",
            tenant: &tenant,
            activated: &Activation::all(),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy: Arc::clone(&policy),
            keyring: None,
        },
    )
    .expect("the plane assembles");
    let cfg: agentd::config::AgentdConfig = toml::from_str(&format!(
        r#"
tenant = "{TENANT}"
allow_insecure_no_auth = true
mcp_api_key = "test"
[journal]
backend = "redb"
path = "/nonexistent/journal.redb"
[mcp_servers]
[providers.anthropic]
backend = "anthropic"
api_key = "test"
"#
    ))
    .expect("the configuration parses");
    let secrets = cfg.resolve_secrets().expect("literal secrets");
    Arc::new(AppState {
        cfg: Arc::new(cfg),
        secrets,
        plane,
        policy,
    })
}

fn person(sub: &str, roles: &[&str]) -> Claims {
    Claims(JwtClaims {
        sub: sub.to_owned(),
        mako_tenant: Some(TENANT.to_owned()),
        mako_roles: roles.iter().map(|r| (*r).to_owned()).collect(),
        mako_sparte: Vec::new(),
    })
}

fn erika() -> Claims {
    person("erika", &["marktkommunikation"])
}

async fn json_of(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Erika asks the desk assistant; the run id.
async fn ask(state: &Arc<AppState>) -> String {
    let req: handlers::RunRequest = serde_json::from_value(json!({
        "agent": "desk-assistant",
        "input": { "frage": "Wie steht die Anmeldung?", "malo": "51238696012" },
    }))
    .expect("a request");
    let response = handlers::start_runs(
        State(Arc::clone(state)),
        erika(),
        HeaderMap::new(),
        mako_service::Json(req),
    )
    .await
    .into_response();
    let (status, body) = json_of(response).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body[0]["run_id"].as_str().expect("a run id").to_owned()
}

/// One SSE event: its name, id and data.
#[derive(Debug, Clone, PartialEq)]
struct Sse {
    event: String,
    id: u64,
    data: Value,
}

/// A `schritte` stream, read event by event.
struct Folge {
    body: axum::body::BodyDataStream,
    buf: String,
}

impl Folge {
    /// The next event, or `None` when the stream ended.
    async fn next(&mut self) -> Option<Sse> {
        loop {
            if let Some(end) = self.buf.find("\n\n") {
                let block: String = self.buf.drain(..end + 2).collect();
                let (mut event, mut id, mut data) = (None, None, None);
                for line in block.lines() {
                    if let Some(v) = line.strip_prefix("event:") {
                        event = Some(v.trim().to_owned());
                    } else if let Some(v) = line.strip_prefix("id:") {
                        id = Some(v.trim().parse().expect("a numeric id"));
                    } else if let Some(v) = line.strip_prefix("data:") {
                        data = Some(serde_json::from_str(v.trim()).expect("json data"));
                    }
                }
                // A keep-alive comment carries no event.
                if let (Some(event), Some(id), Some(data)) = (event, id, data) {
                    return Some(Sse { event, id, data });
                }
                continue;
            }
            let chunk = tokio::time::timeout(Duration::from_secs(10), self.body.next())
                .await
                .expect("the stream stalled")?
                .expect("a body chunk");
            self.buf
                .push_str(std::str::from_utf8(&chunk).expect("utf-8"));
        }
    }

    /// Every remaining event, the stream required to end.
    async fn rest(&mut self) -> Vec<Sse> {
        let mut out = Vec::new();
        while let Some(e) = self.next().await {
            out.push(e);
        }
        out
    }
}

async fn follow(
    state: &Arc<AppState>,
    who: Claims,
    run: &str,
    last: Option<u64>,
) -> Result<Folge, (StatusCode, Value)> {
    let mut headers = HeaderMap::new();
    if let Some(last) = last {
        headers.insert("last-event-id", HeaderValue::from(last));
    }
    match handlers::schritte(State(Arc::clone(state)), who, headers, Path(run.to_owned())).await {
        Ok(response) => {
            assert_eq!(
                response.headers()["content-type"],
                "text/event-stream",
                "an SSE response"
            );
            Ok(Folge {
                body: response.into_body().into_data_stream(),
                buf: String::new(),
            })
        }
        Err(e) => Err(json_of(e.into_response()).await),
    }
}

async fn stop(state: &Arc<AppState>, who: Claims, run: &str) -> (StatusCode, Value) {
    match handlers::stopp(State(Arc::clone(state)), who, Path(run.to_owned()), None).await {
        Ok(ok) => json_of(ok.into_response()).await,
        Err(e) => json_of(e.into_response()).await,
    }
}

async fn answer_of(state: &Arc<AppState>, run: &str) -> Value {
    serde_json::to_value(
        state
            .plane
            .answer(run, "erika")
            .await
            .expect("readable")
            .expect("known"),
    )
    .expect("serialisable")
}

#[tokio::test]
async fn a_runs_reads_stream_in_order_then_its_answer_and_a_reconnect_resumes() {
    let tools = Arc::new(Makod {
        blocks: Some("get_process"),
        ..Makod::default()
    });
    let provider = FakeProvider::new();
    provider.will_answer(read(
        "c1",
        "get_process_history",
        json!({ "process_id": "4711" }),
    ));
    provider.will_answer(read(
        "c2",
        "get_process",
        json!({ "business_key": "51238696012" }),
    ));
    provider.will_answer(answered());
    let state = state(Arc::clone(&tools), &provider);
    let run = ask(&state).await;

    // The first read arrives while the run is still working on the second.
    let mut live = follow(&state, erika(), &run, None).await.expect("followed");
    let first = live.next().await.expect("a first step");
    tools.entered.notified().await;
    assert_eq!(answer_of(&state, &run).await["status"], "running");
    assert_eq!(first.event, "schritt");
    assert_eq!(
        first.data["seq"], first.id,
        "the id is the journal sequence"
    );
    assert_eq!(first.data["art"], "lesen");
    assert_eq!(first.data["was"], "gelesen");
    assert_eq!(first.data["objekt"], "Prozess 4711");
    assert_eq!(
        first.data["quelle"],
        json!({ "dienst": "makod", "werkzeug": "get_process_history", "ref": "4711" })
    );
    assert_eq!(first.data["verborgen"], false);
    assert!(
        time::OffsetDateTime::parse(
            first.data["um"].as_str().expect("a time"),
            &time::format_description::well_known::Rfc3339
        )
        .is_ok(),
        "{first:?}"
    );

    tools.release.notify_one();
    let rest = live.rest().await;
    let names: Vec<&str> = rest.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(names, ["schritt", "antwort"], "{rest:#?}");
    assert_eq!(rest[0].data["quelle"]["werkzeug"], "get_process");
    assert_eq!(rest[0].data["objekt"], "Prozessschlüssel 51238696012");
    assert!(
        first.id < rest[0].id && rest[0].id < rest[1].id,
        "in journal order"
    );

    // The final event is the answer read, field for field.
    assert_eq!(rest[1].data, answer_of(&state, &run).await);
    assert_eq!(rest[1].data["status"], "succeeded");

    // A reconnect after the first step sends the rest once, under the same ids.
    let resumed = follow(&state, erika(), &run, Some(first.id))
        .await
        .expect("followed")
        .rest()
        .await;
    // `um` is when an instance read the record — the journal stamps no time —
    // so it is the one field a second read does not repeat.
    let ohne_um = |events: &[Sse]| -> Vec<Sse> {
        events
            .iter()
            .cloned()
            .map(|mut e| {
                if let Some(o) = e.data.as_object_mut() {
                    o.remove("um");
                }
                e
            })
            .collect()
    };
    assert_eq!(
        ohne_um(&resumed),
        ohne_um(&rest),
        "no step missed, none repeated"
    );
    // One that already holds the conclusion is told nothing more.
    let over = follow(&state, erika(), &run, Some(rest[1].id))
        .await
        .expect("followed")
        .rest()
        .await;
    assert!(over.is_empty(), "{over:#?}");

    // A concluded run's stop is refused: its answer stands.
    let (status, body) = stop(&state, erika(), &run).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "already_concluded", "{body}");
    assert_eq!(answer_of(&state, &run).await["status"], "succeeded");
}

#[tokio::test]
async fn a_stopped_run_reads_nothing_after_the_stop_and_its_stream_ends_abgebrochen() {
    let tools = Arc::new(Makod {
        blocks: Some("get_process_history"),
        ..Makod::default()
    });
    let provider = FakeProvider::new();
    provider.will_answer(read(
        "c1",
        "get_process_history",
        json!({ "process_id": "4711" }),
    ));
    // What the model asks after the stop: never read.
    provider.will_answer(read(
        "c2",
        "get_process",
        json!({ "business_key": "51238696012" }),
    ));
    provider.will_answer(answered());
    let state = state(Arc::clone(&tools), &provider);
    let run = ask(&state).await;
    let mut live = follow(&state, erika(), &run, None).await.expect("followed");

    // Another person cannot stop it here.
    tools.entered.notified().await;
    let (status, body) = stop(&state, person("jan", &["marktkommunikation"]), &run).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "not_the_starter", "{body}");

    // Erika stops it while its first read is in flight.
    let (status, body) = stop(&state, erika(), &run).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["recorded"], true, "{body}");
    tools.release.notify_one();

    let events = live.rest().await;
    let last = events.last().expect("an end");
    assert_eq!(last.event, "abgebrochen", "{events:#?}");
    assert_eq!(last.data["durch"], "erika");
    let read_after_stop: Vec<&Sse> = events
        .iter()
        .filter(|e| e.event == "schritt" && e.data["art"] == "lesen")
        .filter(|e| e.data["quelle"]["werkzeug"] != "get_process_history")
        .collect();
    assert!(read_after_stop.is_empty(), "{events:#?}");
    assert_eq!(
        *tools.calls.lock().unwrap(),
        ["get_process_history"],
        "no service is read after the stop"
    );
    assert_eq!(answer_of(&state, &run).await["status"], "cancelled");
}

#[tokio::test]
async fn only_the_starter_with_the_verb_follows_or_stops_a_run() {
    let tools = Arc::new(Makod::default());
    let provider = FakeProvider::new();
    provider.will_answer(answered());
    let state = state(tools, &provider);
    let run = ask(&state).await;

    // Authenticated, but holding no role that admits reading or starting runs.
    let nobody = || person("erika", &[]);
    let (status, _) = follow(&state, nobody(), &run, None)
        .await
        .err()
        .expect("refused");
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = stop(&state, nobody(), &run).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Holding the verbs, but not the one who started it.
    let jan = || person("jan", &["marktkommunikation"]);
    let (status, body) = follow(&state, jan(), &run, None)
        .await
        .err()
        .expect("refused");
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "not_the_starter", "{body}");

    // No such run.
    let unknown = agentplane::core::RunId::generate().to_string();
    let (status, _) = follow(&state, erika(), &unknown, None)
        .await
        .err()
        .expect("refused");
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = stop(&state, erika(), &unknown).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // An event's run has no starter: anyone holding `RUN_READ` follows it,
    // and nobody stops it here — the oversight surface does.
    let accepted = state
        .plane
        .accept_named(
            "desk-assistant",
            agentd::plane::Envelope {
                id: "ev-1",
                source: "urn:mako:test",
                event_type: "manual",
            },
            json!({ "frage": "?", "malo": "51238696012" }),
        )
        .await
        .expect("activated");
    let events = follow(&state, jan(), &accepted.run_id, None)
        .await
        .map_err(|(s, b)| format!("{s} {b}"))
        .expect("a starter-less run is followed under RUN_READ")
        .rest()
        .await;
    assert_eq!(
        events.last().map(|e| e.event.as_str()),
        Some("antwort"),
        "{events:#?}"
    );
    let (status, _) = follow(&state, nobody(), &accepted.run_id, None)
        .await
        .err()
        .expect("refused");
    assert_eq!(status, StatusCode::FORBIDDEN, "RUN_READ still applies");
    let (status, body) = stop(&state, erika(), &accepted.run_id).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "not_the_starter", "{body}");

    // A `Last-Event-ID` this stream never sent is refused, not read as zero.
    let mut headers = HeaderMap::new();
    headers.insert("last-event-id", HeaderValue::from_static("vier"));
    let refused = handlers::schritte(State(Arc::clone(&state)), erika(), headers, Path(run))
        .await
        .expect_err("refused");
    assert_eq!(
        refused.into_response().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

async fn answer_read(state: &Arc<AppState>, who: Claims, run: &str) -> (StatusCode, Value) {
    match handlers::answer(State(Arc::clone(state)), who, Path(run.to_owned())).await {
        Ok(ok) => json_of(ok.into_response()).await,
        Err(e) => json_of(e.into_response()).await,
    }
}

#[tokio::test]
async fn a_started_runs_answer_is_its_starters_and_a_starterless_one_is_the_tenants() {
    let tools = Arc::new(Makod::default());
    let provider = FakeProvider::new();
    provider.will_answer(answered());
    provider.will_answer(answered());
    let state = state(tools, &provider);
    let jan = || person("jan", &["marktkommunikation"]);

    // Erika's own question: hers to read, nobody else's.
    let run = ask(&state).await;
    let (status, body) = answer_read(&state, erika(), &run).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["run"], run.as_str());
    let (status, body) = answer_read(&state, jan(), &run).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "not_the_starter", "{body}");

    // A waiting proposal is admitted by the plane, with no starter: Heute
    // reads it for everyone holding RUN_READ, by its answer and by
    // `vorschlaege`.
    let proposal = state
        .plane
        .vorschlag_zulassen(agentd::plane::vorschlag::Art::Freigabe, "e1")
        .await
        .expect("the desk assistant is activated");
    assert!(proposal.refused.is_none(), "{:?}", proposal.refused);
    for who in [erika(), jan()] {
        let (status, body) = answer_read(&state, who, &proposal.run_id).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let q: handlers::VorschlaegeQuery =
        serde_json::from_value(json!({ "freigabe": "e1" })).expect("a query");
    let response = handlers::vorschlaege(State(Arc::clone(&state)), jan(), axum::extract::Query(q))
        .await
        .expect("read");
    let (status, body) = json_of(response.into_response()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["freigabe"]["e1"]["run"],
        proposal.run_id.as_str(),
        "{body}"
    );
}
