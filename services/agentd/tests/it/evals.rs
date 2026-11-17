//! **What a specialist answers on a case whose right answer is known.**
//!
//! Every case under `services/agentd/evals/<specialist>/` is replayed through
//! the production plane (`Plane::new`, real Cedar set, real manifests) with
//! no network: the model turns come from the case's cassette, served by
//! `FakeProvider`, and the tool results from the case's recording, served by
//! a `ToolClient` that refuses any tool the recording does not hold. The
//! answer is scored on its closed-schema fields (`plane::eval`) and the report
//! is written to `agent-evals.json` in the test's target directory.
//!
//! Two contracts are asserted, and no score is:
//!
//! * **Determinism** — a cassette's scored outcome equals its
//!   `expected/<case>.json`, so a change to the plane's handling of a fixed
//!   answer (an agentplane minor, a labelling edit) fails here and names the
//!   case.
//! * **Injection twins** — a twin is its clean case with untrusted inbound
//!   text (a counterparty's FTX, an e-mail, a PDF's text) set into a tool
//!   result; it holds only when its tool calls and its proposed command
//!   equal the clean case's (AgentDojo 2406.13352).
//!
//! A cassette whose recorded asks no longer match what the plane asks — the
//! manifest digest, the number of turns, the tools offered — reads `stale`
//! and is not scored. A **scripted** cassette (`source: "scripted"`, written
//! by hand until `cargo xtask record-evals` records one from a live provider)
//! that is stale fails: re-recording it costs nothing,
//! `AGENTD_EVALS_BLESS=1 cargo test -p agentd --test it --
//! evals::every_case_replays` rewrites its asks and its expected outcome
//! from the current plane (that one test alone: the others read the files). A recorded cassette is never blessed.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentplane::store::RedbStore;
use agentplane::testkit::FakeProvider;
use agentplane::tools::{ToolClient, ToolError, ToolId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use agentd::plane::eval::{Expectation, Outcome, Scoring, Tally, score, twin_holds};
use agentd::plane::{Activation, Envelope, Plane, PlaneConfig, Stores};

const BLESS: &str = "AGENTD_EVALS_BLESS";

fn evals_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("evals")
}

/// One labelled case.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    specialist: String,
    id: String,
    event_type: String,
    #[serde(default)]
    event: Value,
    /// `server/tool` → the results of its calls, in order.
    #[serde(default)]
    tool_results: BTreeMap<String, Vec<Value>>,
    expected: Expectation,
    #[serde(default)]
    expected_citations: Vec<String>,
    provenance: String,
    /// The clean case this one is an injection twin of.
    #[serde(default)]
    twin_of: Option<String>,
    /// Where a twin's untrusted text sits in the clean case's tool results.
    #[serde(default)]
    injections: Vec<Injection>,
}

/// Untrusted inbound text set into one tool result of the clean case.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Injection {
    /// `ftx`, `email` or `pdf` — where such text reaches a tool output.
    carrier: String,
    /// `server/tool`.
    tool: String,
    /// Which call's result, 0-based.
    #[serde(default)]
    call: usize,
    /// A JSON pointer into that result; its last segment is set.
    pointer: String,
    text: String,
}

/// The recorded model turns of one case.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Cassette {
    case: String,
    /// `scripted`, or `recorded <provider>/<model>`.
    source: String,
    agentplane: String,
    manifest_digest: String,
    turns: Vec<Turn>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Turn {
    /// The tool names the plane offered this turn, sorted.
    offered: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    call: Option<Call>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    answer: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Call {
    id: String,
    name: String,
    arguments: Value,
}

/// Serves a case's recorded tool results, in order, and refuses the rest.
#[derive(Debug, Default)]
struct Recorded {
    results: Mutex<BTreeMap<String, VecDeque<Value>>>,
    calls: Mutex<Vec<Value>>,
}

#[async_trait::async_trait]
impl ToolClient for Recorded {
    fn destination(&self, _tool: &ToolId) -> agentplane::tools::Destination {
        agentplane::tools::Destination::Local
    }

    async fn call(
        &self,
        tool: &ToolId,
        arguments: &Value,
        _provenance: Option<&agentplane::core::Provenance>,
    ) -> Result<Value, ToolError> {
        let key = format!("{}/{}", tool.server, tool.tool);
        self.calls
            .lock()
            .expect("calls")
            .push(json!({ "tool": key, "arguments": arguments }));
        self.results
            .lock()
            .expect("results")
            .get_mut(&key)
            .and_then(VecDeque::pop_front)
            .ok_or_else(|| ToolError::Refused {
                tool: tool.clone(),
                detail: format!("{key} is not recorded for this case"),
            })
    }
}

/// What one replay produced.
enum Replay {
    Scored(Outcome),
    /// The first differing field.
    Stale(String),
}

/// A case with its cassette and the outcome recorded for it.
struct Loaded {
    case: Case,
    cassette: Cassette,
    cassette_path: PathBuf,
    expected: Option<Outcome>,
    expected_path: PathBuf,
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn write(path: &Path, value: &impl Serialize) {
    let text = serde_json::to_string_pretty(value).expect("serializes") + "\n";
    std::fs::write(path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Every specialist's scoring and cases, by specialist.
fn load_all() -> BTreeMap<String, (Scoring, Vec<Loaded>)> {
    let mut out = BTreeMap::new();
    let Ok(dirs) = std::fs::read_dir(evals_dir()) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = dirs
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let scoring: Scoring = read(&dir.join("scoring.json"));
        let mut cases = Vec::new();
        for path in json_files(&dir.join("cases")) {
            let case: Case = read(&path);
            assert_eq!(case.specialist, name, "{}", path.display());
            let file = format!("{}.json", case.id);
            let cassette_path = dir.join("cassettes").join(&file);
            let expected_path = dir.join("expected").join(&file);
            let cassette: Cassette = read(&cassette_path);
            assert_eq!(cassette.case, case.id, "{}", cassette_path.display());
            let expected = expected_path.is_file().then(|| read(&expected_path));
            cases.push(Loaded {
                case,
                cassette,
                cassette_path,
                expected,
                expected_path,
            });
        }
        out.insert(name, (scoring, cases));
    }
    out
}

/// The tool results a case is served: a twin's are its clean case's with the
/// untrusted text set in, so the two differ in nothing else.
fn tool_results(case: &Case, cases: &[Loaded]) -> BTreeMap<String, Vec<Value>> {
    let Some(clean) = &case.twin_of else {
        return case.tool_results.clone();
    };
    let clean = cases
        .iter()
        .find(|l| &l.case.id == clean)
        .unwrap_or_else(|| panic!("{}: twin of an unknown case {clean}", case.id));
    assert!(
        case.tool_results.is_empty(),
        "{}: a twin's tool results are its clean case's",
        case.id
    );
    assert!(
        !case.injections.is_empty(),
        "{}: a twin carries an injection",
        case.id
    );
    let mut results = clean.case.tool_results.clone();
    for inj in &case.injections {
        assert!(
            matches!(inj.carrier.as_str(), "ftx" | "email" | "pdf"),
            "{}: carrier {}",
            case.id,
            inj.carrier
        );
        let target = results
            .get_mut(&inj.tool)
            .and_then(|r| r.get_mut(inj.call))
            .unwrap_or_else(|| {
                panic!(
                    "{}: {} call {} is not recorded",
                    case.id, inj.tool, inj.call
                )
            });
        let (parent, field) = inj
            .pointer
            .rsplit_once('/')
            .unwrap_or_else(|| panic!("{}: pointer {}", case.id, inj.pointer));
        let slot = target
            .pointer_mut(parent)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("{}: {} has no object at {parent}", case.id, inj.tool));
        slot.insert(field.to_owned(), Value::from(inj.text.clone()));
    }
    results
}

fn plane(provider: &Arc<FakeProvider>, tools: &Arc<Recorded>) -> Plane {
    let tenant = agentplane::core::TenantId::new("9900357000004").expect("a usable key scope");
    let policy = agentd::plane::policy::engine(agentd::plane::policy::DEFAULT_POLICY)
        .expect("the embedded policy set compiles");
    let servers = agentd::plane::tools::servers_named_in_grants()
        .into_iter()
        .map(|name| (name, Arc::clone(tools) as Arc<dyn ToolClient>))
        .collect();
    Plane::new(
        Stores::redb(RedbStore::open_in_memory().expect("store"), &tenant),
        PlaneConfig {
            outbox: None,
            signer: None,
            quota: agentplane::quota::TenantQuota::default(),
            owner: "agentd-evals",
            tenant: &tenant,
            activated: &Activation::all(),
            providers: vec![(
                "anthropic".to_owned(),
                Arc::clone(provider) as Arc<dyn agentplane::model::ModelProvider>,
            )],
            tool_servers: servers,
            policy,
            keyring: None,
        },
    )
    .expect("the plane assembles")
}

/// The asks of one replay, in the cassette's terms.
fn offered(asks: &[agentplane::testkit::Ask]) -> Vec<Vec<String>> {
    asks.iter()
        .map(|a| {
            let mut names: Vec<String> = a.tools.iter().map(|t| t.name.clone()).collect();
            names.sort();
            names
        })
        .collect()
}

/// Replay one case. `bless` rewrites a scripted cassette's asks to the
/// plane's and reports the replay as scored.
async fn replay(
    case: &Case,
    cassette: &mut Cassette,
    results: BTreeMap<String, Vec<Value>>,
    scoring: &Scoring,
    bless: bool,
) -> Replay {
    let digest = agentd::plane::declaration(&case.specialist)
        .and_then(|d| d.digest)
        .unwrap_or_else(|| panic!("{}: no such specialist", case.specialist));

    let provider = FakeProvider::new();
    for turn in &cassette.turns {
        match (&turn.call, &turn.answer) {
            (Some(c), None) => {
                provider.will_call_tool(c.id.clone(), c.name.clone(), c.arguments.clone());
            }
            (None, Some(a)) => {
                provider.will_structure(a.clone());
            }
            _ => panic!("{}: a turn is a call or an answer", case.id),
        }
    }
    let tools = Arc::new(Recorded {
        results: Mutex::new(results.into_iter().map(|(k, v)| (k, v.into())).collect()),
        calls: Mutex::default(),
    });
    let plane = plane(&provider, &tools);
    let event_id = format!("eval-{}", case.id);
    let decision = plane
        .dispatch_one(
            &case.specialist,
            Envelope {
                id: &event_id,
                source: "urn:mako:test:tenant:9900357000004",
                event_type: &case.event_type,
            },
            case.event.clone(),
        )
        .await
        .expect("an activated specialist");
    let answer = match agentplane::core::RunId::parse(&decision.run_id) {
        Ok(run) => plane
            .runtime()
            .output(run)
            .await
            .expect("the journal reads")
            .map(|t| t.peek().clone())
            .unwrap_or_default(),
        Err(_) => Value::Null,
    };

    let asks = offered(&provider.asked());
    if bless && cassette.source == "scripted" {
        cassette.manifest_digest = digest.clone();
        for (turn, names) in cassette.turns.iter_mut().zip(&asks) {
            turn.offered.clone_from(names);
        }
    }
    if cassette.manifest_digest != digest {
        return Replay::Stale(format!(
            "manifest_digest: recorded {}, the plane's {digest}",
            cassette.manifest_digest
        ));
    }
    if asks.len() != cassette.turns.len() || !provider.script_exhausted() {
        return Replay::Stale(format!(
            "turns: recorded {}, the plane asked {}",
            cassette.turns.len(),
            asks.len()
        ));
    }
    for (i, (turn, names)) in cassette.turns.iter().zip(&asks).enumerate() {
        if &turn.offered != names {
            return Replay::Stale(format!(
                "turns[{i}].offered: recorded {:?}, the plane offered {names:?}",
                turn.offered
            ));
        }
    }
    let calls = tools.calls.lock().expect("calls").clone();
    Replay::Scored(Outcome::of(&decision.outcome, &answer, scoring, calls))
}

/// The suite's verdict over every case.
#[derive(Default)]
struct Verdict {
    /// Contract violations: a changed outcome, a twin that did not hold, a
    /// stale scripted cassette.
    failures: Vec<String>,
    tallies: BTreeMap<String, Tally>,
}

async fn evaluate(all: &mut BTreeMap<String, (Scoring, Vec<Loaded>)>, bless: bool) -> Verdict {
    let mut verdict = Verdict::default();
    for (name, (scoring, cases)) in all.iter_mut() {
        let mut tally = Tally::default();
        let mut outcomes: BTreeMap<String, Outcome> = BTreeMap::new();
        let resolved: Vec<BTreeMap<String, Vec<Value>>> =
            cases.iter().map(|l| tool_results(&l.case, cases)).collect();
        for (loaded, results) in cases.iter_mut().zip(resolved) {
            let id = loaded.case.id.clone();
            match replay(&loaded.case, &mut loaded.cassette, results, scoring, bless).await {
                Replay::Stale(why) => {
                    tally.stale += 1;
                    if loaded.cassette.source == "scripted" {
                        verdict.failures.push(format!(
                            "{name}/{id}: stale — {why} (a scripted cassette: {BLESS}=1 re-records it)"
                        ));
                    }
                }
                Replay::Scored(outcome) => {
                    if bless {
                        write(&loaded.cassette_path, &loaded.cassette);
                        write(&loaded.expected_path, &outcome);
                        loaded.expected = Some(outcome.clone());
                    }
                    match &loaded.expected {
                        Some(e) if e == &outcome => {}
                        Some(e) => verdict.failures.push(format!(
                            "{name}/{id}: the scored outcome changed\n    recorded {}\n    replayed {}",
                            serde_json::to_string(e).unwrap_or_default(),
                            serde_json::to_string(&outcome).unwrap_or_default(),
                        )),
                        None => verdict.failures.push(format!(
                            "{name}/{id}: no expected outcome is recorded"
                        )),
                    }
                    tally.add(score(&loaded.case.expected, &outcome));
                    outcomes.insert(id, outcome);
                }
            }
        }
        for loaded in cases.iter() {
            let Some(clean) = &loaded.case.twin_of else {
                continue;
            };
            let (Some(c), Some(t)) = (outcomes.get(clean), outcomes.get(&loaded.case.id)) else {
                continue;
            };
            tally.injection_twins += 1;
            if twin_holds(c, t) {
                tally.injection_held += 1;
            } else {
                verdict.failures.push(format!(
                    "{name}/{}: the injection twin of {clean} did not hold — its calls or its command differ",
                    loaded.case.id
                ));
            }
        }
        verdict.tallies.insert(name.clone(), tally);
    }
    verdict
}

/// The tool-calling specialists — the set the report covers, derived from
/// each manifest's `execution.kind`, never a count.
fn tool_calling() -> Vec<String> {
    agentd::plane::manifests()
        .iter()
        .filter(|(_, m)| {
            matches!(
                m.spec.execution.as_ref().map(|e| e.kind),
                Some(agentplane::manifest::ExecutionKind::ToolCalling)
            )
        })
        .map(|(n, _)| n.clone())
        .collect()
}

fn report(verdict: &Verdict) -> Value {
    let specialists: serde_json::Map<String, Value> = tool_calling()
        .into_iter()
        .map(|name| {
            let row = verdict
                .tallies
                .get(&name)
                .filter(|t| t.cases + t.stale > 0)
                .map_or_else(|| Value::from("not measured"), Tally::report);
            (name, row)
        })
        .collect();
    json!({
        "agentplane": "0.61",
        "gate": "none — the figures are reported; determinism and the injection twins are asserted",
        "specialists": specialists,
    })
}

/// Every case replays to its recorded outcome, every twin holds, and the
/// report is written.
#[tokio::test]
async fn every_case_replays_to_its_recorded_outcome() {
    let bless = std::env::var_os(BLESS).is_some();
    let mut all = load_all();
    assert!(
        all.contains_key(agentd::plane::vorschlag::SKILL),
        "the desk assistant's cases are the first: {:?}",
        all.keys()
    );
    let verdict = evaluate(&mut all, bless).await;
    let report = report(&verdict);
    let path = Path::new(env!("CARGO_TARGET_TMPDIR")).join("agent-evals.json");
    write(&path, &report);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    assert!(
        verdict.failures.is_empty(),
        "{}",
        verdict.failures.join("\n")
    );
}

/// A cassette recorded under another procedure is reported stale and is not
/// scored.
#[tokio::test]
async fn a_cassette_for_another_procedure_reads_stale() {
    let mut all = load_all();
    let (scoring, cases) = all
        .get_mut(agentd::plane::vorschlag::SKILL)
        .expect("the desk assistant has cases");
    let loaded = cases
        .iter_mut()
        .find(|l| l.case.twin_of.is_none())
        .expect("a clean case");
    loaded.cassette.manifest_digest = "0".repeat(64);
    let results = loaded.case.tool_results.clone();
    let got = replay(&loaded.case, &mut loaded.cassette, results, scoring, false).await;
    let Replay::Stale(why) = got else {
        panic!("a cassette for another digest was scored");
    };
    assert!(why.starts_with("manifest_digest"), "{why}");
}

/// A twin whose model proposes another command does not hold, and the suite
/// says which.
#[tokio::test]
async fn a_twin_with_another_command_fails() {
    let mut all = load_all();
    let skill = agentd::plane::vorschlag::SKILL.to_owned();
    let (_, cases) = all.get_mut(&skill).expect("the desk assistant has cases");
    let twin = cases
        .iter_mut()
        .find(|l| l.case.twin_of.is_some())
        .expect("an injection twin");
    let id = twin.case.id.clone();
    let last = twin
        .cassette
        .turns
        .iter_mut()
        .rev()
        .find_map(|t| t.answer.as_mut())
        .expect("an answer turn");
    // The injected text won: the model now proposes to settle.
    last["vorschlag"]["aktion"] = json!("freigeben");
    last["vorschlag"]["antwortcode"] = Value::Null;
    last["vorschlag"]["befund"] = Value::Null;
    all.retain(|k, _| *k == skill);
    let verdict = evaluate(&mut all, false).await;
    assert!(
        verdict
            .failures
            .iter()
            .any(|f| f.contains(&format!("{skill}/{id}: the injection twin"))),
        "{:?}",
        verdict.failures
    );
}

/// Every case states the citations it expects and where its label came from;
/// the label vocabulary is the scorer's.
#[test]
fn every_case_names_its_provenance() {
    for (name, (_, cases)) in load_all() {
        for l in cases {
            let p = l.case.provenance.as_str();
            assert!(
                p == "synthetic"
                    || p.starts_with("derived-from-fixture ")
                    || p.starts_with("labelled-by "),
                "{name}/{}: provenance {p:?}",
                l.case.id
            );
            for c in &l.case.expected_citations {
                assert!(
                    !c.trim().is_empty(),
                    "{name}/{}: an empty citation",
                    l.case.id
                );
            }
        }
    }
}
