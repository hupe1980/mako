//! The reference inventory, established before the first target request.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::document::Document;
use crate::report::Runner;
use crate::suite::Suite;

/// Subjects come from the committed kit. A response cannot introduce a shareable ID.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum Probe {
    RestAuth(String),
    RestStatus(String),
    RestBody(String),
    RestRefusal(String),
    Listener,
    Register,
    TestRequest,
    TestDelivery,
    Seed,
    ReplayRequest,
    ReplayEnvelope,
    Cleanup,
    Event(String),
    Inbound(String),
    McpAuth,
    Tools,
    Tool(String),
    McpRefusal(String),
}

impl Probe {
    pub(crate) fn runner(&self) -> Runner {
        match self {
            Self::RestAuth(_) | Self::RestStatus(_) | Self::RestBody(_) | Self::RestRefusal(_) => {
                Runner::Rest
            }
            Self::McpAuth | Self::Tools | Self::Tool(_) | Self::McpRefusal(_) => Runner::Mcp,
            _ => Runner::Events,
        }
    }

    pub(crate) fn id(&self) -> String {
        match self {
            Self::RestAuth(op) => format!("rest.auth:{op}"),
            Self::RestStatus(op) => format!("rest.status:{op}"),
            Self::RestBody(op) => format!("rest.body:{op}"),
            Self::RestRefusal(path) => format!("rest.refusal:GET {path}"),
            Self::Listener => "events.listener".into(),
            Self::Register => "events.register".into(),
            Self::TestRequest => "events.test_request".into(),
            Self::TestDelivery => "events.test_delivery".into(),
            Self::Seed => "events.seed".into(),
            Self::ReplayRequest => "events.replay_request".into(),
            Self::Cleanup => "events.cleanup".into(),
            Self::ReplayEnvelope => "events.replay_envelope".into(),
            Self::Event(ty) => format!("events.schema:{ty}"),
            Self::Inbound(path) => format!("events.inbound:{path}"),
            Self::McpAuth => "mcp.auth".into(),
            Self::Tools => "mcp.tools".into(),
            Self::Tool(tool) => format!("mcp.tool:{tool}"),
            Self::McpRefusal(tool) => format!("mcp.refusal:{tool}"),
        }
    }

    pub(crate) fn name(&self) -> String {
        match self {
            Self::RestAuth(op) => format!("401 without a token: {op}"),
            Self::RestStatus(op) => format!("documented status: {op}"),
            Self::RestBody(op) => format!("documented answer: {op}"),
            Self::RestRefusal(path) => format!("403 for the refused principal: GET {path}"),
            Self::TestDelivery => "the test delivery is signed and a CloudEvent".into(),
            Self::Tools => "tools/list names the declared tools".into(),
            Self::Inbound(path) => format!("inbound {path}"),
            Self::Event(ty) => format!("event {ty}"),
            _ => self.id(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Profile {
    pub(crate) service: String,
    pub(crate) only: Option<Runner>,
    pub(crate) expected: BTreeMap<Probe, bool>,
    pub(crate) fingerprint: String,
    pub(crate) materials: BTreeMap<String, Option<String>>,
}

impl Profile {
    pub(crate) fn new(
        suite: &dyn Suite,
        only: Option<Runner>,
        document: &Document,
        openapi: &str,
        snapshot: &str,
        tools: &[String],
        schemas: &[(&str, &str)],
    ) -> anyhow::Result<Self> {
        let mut expected = BTreeMap::new();
        let mut inputs = BTreeMap::new();
        for (module, source) in [
            ("run", include_str!("lib.rs")),
            ("suite", include_str!("suite.rs")),
            ("profile", include_str!("profile.rs")),
            ("rest", include_str!("rest.rs")),
            ("events", include_str!("events.rs")),
            ("mcp", include_str!("mcp.rs")),
            ("report", include_str!("report.rs")),
            ("document", include_str!("document.rs")),
            ("transport", include_str!("transport.rs")),
        ] {
            inputs.insert(format!("kit.{module}"), Some(source.to_owned()));
        }
        inputs.insert("profile.revision".into(), Some("1".to_owned()));
        inputs.insert("kit.version".into(), Some(env!("CARGO_PKG_VERSION").into()));
        inputs.insert("suite.service".into(), Some(suite.service().into()));
        inputs.insert("suite.definition".into(), Some(suite.definition().into()));
        inputs.insert("openapi".into(), Some(openapi.into()));
        inputs.insert("mcp.snapshot".into(), Some(snapshot.into()));
        let mut add = |probe: Probe, required: bool| -> anyhow::Result<()> {
            let id = probe.id();
            anyhow::ensure!(
                expected.insert(probe, required).is_none(),
                "duplicate reference probe {id}"
            );
            inputs.insert(format!("probe:{id}"), Some(required.to_string()));
            Ok(())
        };
        for op in document.operations() {
            if op.protected() {
                add(Probe::RestAuth(op.name()), true)?;
            }
            if op.method == "GET" && !op.has_path_params() && !op.requires_query() {
                add(Probe::RestStatus(op.name()), true)?;
                add(Probe::RestBody(op.name()), op.declares_success_json())?;
            }
        }
        let refusal = suite.refusal();
        add(Probe::RestRefusal(refusal.path.into()), true)?;
        add(Probe::McpRefusal(refusal.tool.into()), true)?;
        for probe in [
            Probe::Listener,
            Probe::Register,
            Probe::TestRequest,
            Probe::TestDelivery,
            Probe::Seed,
            Probe::Cleanup,
            Probe::ReplayRequest,
            Probe::ReplayEnvelope,
            Probe::McpAuth,
            Probe::Tools,
        ] {
            add(probe, true)?;
        }
        for tool in tools {
            add(Probe::Tool(tool.clone()), true)?;
        }
        let inbound = suite.inbound();
        for route in &inbound {
            add(Probe::Inbound(route.path.into()), true)?;
        }
        let event_types = suite.emits();
        for ty in &event_types {
            add(Probe::Event((*ty).into()), true)?;
        }
        inputs.insert(
            "suite.refusal.arguments".into(),
            Some(refusal.arguments.to_string()),
        );
        for route in inbound {
            inputs.insert(
                format!("suite.inbound:{}", route.path),
                Some(route.event.to_string()),
            );
        }
        for ty in event_types {
            let mut matching = schemas.iter().filter(|(name, _)| *name == ty);
            let schema = matching.next().map(|(_, text)| (*text).to_owned());
            anyhow::ensure!(
                matching.next().is_none(),
                "duplicate reference event schema"
            );
            inputs.insert(format!("schema:{ty}"), schema);
        }
        expected.retain(|probe, _| only.is_none_or(|runner| probe.runner() == runner));
        let materials = inputs
            .iter()
            .filter(|(name, _)| !name.starts_with("probe:"))
            .map(|(name, content)| {
                (
                    name.clone(),
                    content
                        .as_ref()
                        .map(|text| format!("sha256:{:x}", Sha256::digest(text.as_bytes()))),
                )
            })
            .collect();
        Ok(Self {
            service: suite.service().into(),
            only,
            expected,
            fingerprint: fingerprint(&inputs),
            materials,
        })
    }
}

fn fingerprint(inputs: &BTreeMap<String, Option<String>>) -> String {
    let mut hash = Sha256::new();
    for (name, contents) in inputs {
        hash.update((name.len() as u64).to_be_bytes());
        hash.update(name.as_bytes());
        hash.update([u8::from(contents.is_some())]);
        if let Some(text) = contents {
            hash.update((text.len() as u64).to_be_bytes());
            hash.update(text.as_bytes());
        }
    }
    format!("sha256:{:x}", hash.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_applicability_comes_from_the_reference_before_any_call() {
        let text = r#"{"openapi":"3.1.0","paths":{
            "/empty":{"get":{"responses":{"204":{"description":"empty"}}}},
            "/body":{"get":{"responses":{"200":{"content":{"application/json":{"schema":true}}},"204":{}}}}
        }}"#;
        let profile = Profile::new(
            crate::suites::suite("vertragd").unwrap(),
            Some(Runner::Rest),
            &Document::parse(text).unwrap(),
            text,
            "[]",
            &[],
            &[],
        )
        .unwrap();
        assert_eq!(
            profile.expected.get(&Probe::RestBody("GET /empty".into())),
            Some(&false)
        );
        assert_eq!(
            profile
                .expected
                .get(&Probe::RestStatus("GET /empty".into())),
            Some(&true)
        );
        assert_eq!(
            profile.expected.get(&Probe::RestBody("GET /body".into())),
            Some(&true)
        );
        assert!(
            profile
                .expected
                .keys()
                .all(|probe| probe.runner() == Runner::Rest)
        );
    }

    #[test]
    fn fingerprint_is_bound_to_the_schema_bytes_and_independent_of_declaration_order() {
        let suite = crate::suites::suite("vertragd").unwrap();
        let events = suite.emits();
        let make = |schemas: &[(&str, &str)], tools: &[String]| {
            let text = r#"{"paths":{},"openapi":"3.1.0"}"#;
            Profile::new(
                suite,
                None,
                &Document::parse(text).unwrap(),
                text,
                "[]",
                tools,
                schemas,
            )
        };
        let first = (events[0], r#"{"type":"number"}"#);
        let second = (events[1], r#"{"type":"object"}"#);
        let tools = ["a".to_owned(), "b".to_owned()];
        let baseline = make(&[first, second], &tools).unwrap();
        assert_eq!(
            baseline.fingerprint,
            make(&[second, first], &["b".into(), "a".into()])
                .unwrap()
                .fingerprint
        );
        let changed = make(&[(events[0], r#"{"type":"string"}"#), second], &tools).unwrap();
        assert_ne!(baseline.fingerprint, changed.fingerprint);
        assert_ne!(
            baseline.materials[&format!("schema:{}", events[0])],
            changed.materials[&format!("schema:{}", events[0])]
        );
        assert_ne!(
            baseline.fingerprint,
            make(&[second], &tools).unwrap().fingerprint
        );
        assert!(make(&[first, first], &tools).is_err());
        assert!(make(&[first], &["a".into(), "a".into()]).is_err());
    }
}
