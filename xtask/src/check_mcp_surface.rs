//! Guard: every MCP server is served through `mako_service::mcp` and speaks the
//! declared revisions.
//!
//! rmcp's default `supported_protocol_versions()` is every revision the SDK
//! knows, and its default Streamable HTTP service keeps a session per
//! `2025-11-25` client. Both defaults are what a new server gets by writing
//! nothing, and both are silent: a revision a later rmcp learns is agreed to
//! by every server without anyone deciding it, and a session-holding server
//! answers a request that reaches another replica with a 4xx. So the guard
//! refuses the defaults rather than checking that a server opted out.
//!
//! One scan over every source under `services/*/src`, one rule per defect:
//!
//! 1. **The declared revisions.** Each `impl … ServerHandler for` returns
//!    `mako_service::mcp::supported_revisions()` from
//!    `supported_protocol_versions`.
//! 2. **No session.** No source builds `StreamableHttpService::new` or names
//!    `LocalSessionManager`; a server is served by `mako_service::mcp::service`,
//!    which runs it statelessly.
//! 3. **No deprecated server capability.** The 2026-07-28 revision deprecates
//!    Logging and replaces `resources/subscribe` (MCP specification
//!    2026-07-28, Key Changes: Deprecated item 1, Major changes item 4); no
//!    server enables either.
//! 4. **The client agrees.** agentd's client connects through
//!    `McpClient::connect`, which runs `server/discover` on 2026-07-28 first —
//!    a hand-started `initialize` naming 2026-07-28 is answered with
//!    2025-11-25, so every connection would negotiate down — and agentd pins
//!    the servers' declared set to the client's `SPOKEN_REVISIONS`. Sampling and
//!    roots are client capabilities; agentplane's `host_info` advertises
//!    neither, and agentd advertises nothing of its own.

use std::path::{Path, PathBuf};

/// The lowest number of `ServerHandler` impls a healthy tree yields — below it
/// the scan has stopped matching and a clean result is about nothing.
const MIN_SERVERS: usize = 11;

/// Where agentd starts its MCP clients.
const AGENTD_CLIENT: &str = "services/agentd/src/plane/tools.rs";

/// The pin of the servers' declared set to the client's.
const REVISION_PIN: &str = "assert_eq!(mako_service::mcp::REVISIONS, McpClient::SPOKEN_REVISIONS)";

/// Server-side capabilities the 2026-07-28 revision deprecates or replaces.
const DEPRECATED_CAPABILITIES: &[(&str, &str)] = &[
    (
        "enable_logging",
        "Logging is deprecated by MCP 2026-07-28; log to OpenTelemetry instead",
    ),
    (
        "enable_resources_subscribe",
        "`resources/subscribe` is replaced by `subscriptions/listen` in MCP 2026-07-28",
    ),
];

pub fn run(workspace_root: &Path) -> bool {
    let mut sources = Vec::new();
    let Ok(services) = std::fs::read_dir(workspace_root.join("services")) else {
        eprintln!("check-mcp-surface: cannot read services/");
        return false;
    };
    for service in services.flatten() {
        collect_sources(&service.path().join("src"), &mut sources);
    }
    sources.sort();

    let mut findings = Vec::new();
    let mut servers = 0;
    for path in &sources {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let (n, mut found) = check_source(&rel, &src);
        servers += n;
        findings.append(&mut found);
    }
    if servers < MIN_SERVERS {
        findings.push(format!(
            "found {servers} `ServerHandler` impl(s) under services/*/src; at least \
             {MIN_SERVERS} are expected. The scan has stopped matching — fix it before \
             trusting a green run."
        ));
    }

    match std::fs::read_to_string(workspace_root.join(AGENTD_CLIENT)) {
        Ok(src) => findings.extend(check_client(AGENTD_CLIENT, &src)),
        Err(e) => findings.push(format!("{AGENTD_CLIENT}: {e}")),
    }

    if findings.is_empty() {
        println!(
            "check-mcp-surface: {servers} `ServerHandler` impls speak the declared revisions, hold no \
             session and enable no deprecated capability; agentd's client agrees"
        );
        return true;
    }
    eprintln!("check-mcp-surface: {} problem(s):", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    false
}

/// The `ServerHandler` impls in one source, and what it does wrong.
fn check_source(rel: &str, src: &str) -> (usize, Vec<String>) {
    let code = blank_comments(src);
    let mut findings = Vec::new();
    let servers = code
        .lines()
        .filter(|l| l.contains("impl") && l.contains("ServerHandler for "))
        .count();
    let declared = code
        .matches("mako_service::mcp::supported_revisions()")
        .count();
    if servers > 0 && (declared < servers || !code.contains("fn supported_protocol_versions")) {
        findings.push(format!(
            "{rel}: {servers} `ServerHandler` impl(s), {declared} returning \
             `mako_service::mcp::supported_revisions()` from `supported_protocol_versions` — \
             the SDK's default agrees to every revision it knows"
        ));
    }
    for (idx, line) in code.lines().enumerate() {
        let at = idx + 1;
        if line.contains("StreamableHttpService::new") || line.contains("LocalSessionManager") {
            findings.push(format!(
                "{rel}:{at}: a hand-built Streamable HTTP service — serve the server with \
                 `mako_service::mcp::service`, which holds no session"
            ));
        }
        for (capability, why) in DEPRECATED_CAPABILITIES {
            if line.contains(capability) {
                findings.push(format!("{rel}:{at}: `{capability}` — {why}"));
            }
        }
    }
    (servers, findings)
}

/// What agentd's client wiring does wrong.
fn check_client(rel: &str, src: &str) -> Vec<String> {
    let code = blank_comments(src);
    let mut findings = Vec::new();
    if !code.contains("McpClient::connect(") || code.contains(".serve(transport)") {
        findings.push(format!(
            "{rel}: agentd must start each MCP client with `McpClient::connect`, which runs \
             `server/discover` on 2026-07-28 first; a hand-started `initialize` negotiates \
             every connection down to 2025-11-25"
        ));
    }
    if !code.contains(REVISION_PIN) {
        findings.push(format!(
            "{rel}: no `{REVISION_PIN}` — nothing holds the servers' declared revisions to \
             the ones agentd's client speaks"
        ));
    }
    for capability in ["enable_sampling", "enable_roots", "enable_elicitation"] {
        if code.contains(capability) {
            findings.push(format!(
                "{rel}: `{capability}` — the client advertises only what agentplane's \
                 `host_info` implements"
            ));
        }
    }
    findings
}

/// Blank comment lines, keeping newlines, so prose naming a forbidden call is
/// not a call.
fn blank_comments(src: &str) -> String {
    src.lines()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DECLARED: &str = "
impl ServerHandler for Handler {
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        mako_service::mcp::supported_revisions()
    }
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }
}
pub fn router() -> Router {
    let service = mako_service::mcp::service(|| Ok(Handler), StreamableHttpServerConfig::default());
    Router::new().route_service(\"/mcp\", service)
}
";

    #[test]
    fn a_server_through_the_shared_surface_passes() {
        let (servers, findings) = check_source("svc/mcp_server.rs", DECLARED);
        assert_eq!(servers, 1);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_server_without_the_declared_revisions_fails() {
        let src = DECLARED.replace(
            "    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        mako_service::mcp::supported_revisions()
    }
",
            "",
        );
        let (_, findings) = check_source("svc/mcp_server.rs", &src);
        assert_eq!(findings.len(), 1, "{findings:?}");
    }

    #[test]
    fn a_server_enabling_logging_fails() {
        let src = DECLARED.replace(".enable_tools()", ".enable_tools().enable_logging()");
        let (_, findings) = check_source("svc/mcp_server.rs", &src);
        assert!(
            findings.iter().any(|f| f.contains("enable_logging")),
            "{findings:?}"
        );
    }

    #[test]
    fn a_session_holding_service_fails() {
        let src = DECLARED.replace(
            "mako_service::mcp::service(|| Ok(Handler), StreamableHttpServerConfig::default())",
            "StreamableHttpService::new(|| Ok(Handler), LocalSessionManager::default().into(), \
             StreamableHttpServerConfig::default())",
        );
        let (_, findings) = check_source("svc/mcp_server.rs", &src);
        assert!(
            findings.iter().any(|f| f.contains("hand-built")),
            "{findings:?}"
        );
    }

    #[test]
    fn a_commented_call_is_not_a_call() {
        let src = format!("{DECLARED}\n// .enable_logging() is deprecated\n");
        let (_, findings) = check_source("svc/mcp_server.rs", &src);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn a_client_started_by_hand_fails() {
        let good = format!("McpClient::connect(name, transport, destination)\n{REVISION_PIN};\n");
        assert!(check_client("tools.rs", &good).is_empty());
        let by_hand = format!("host_info.serve(transport)\n{REVISION_PIN};\n");
        assert_eq!(check_client("tools.rs", &by_hand).len(), 1);
        let unpinned = "McpClient::connect(name, transport, destination)\n";
        assert_eq!(check_client("tools.rs", unpinned).len(), 1);
    }
}
